#include "vhidinput.h"

// The single most useful teardown fact: whether VHF called us back for
// EvtVhfCleanup, i.e. whether the VHF device really went away. Written from
// that callback, which can run at DISPATCH_LEVEL, hence the plain interlocked
// counter on the read side.
static volatile LONG g_VhfCleanupCount;

// Delivery counters. The whole "submit returns success but the cursor never
// moves" question reduces to where between the IOCTL and VHF a report goes:
//   SubmitIo  - IOCTLs that reached VhidInputSubmitReport
//   SubmitQ   - reports actually enqueued
//   PumpCalls - times VhidInputPumpLocked ran
//   PumpOk    - VhfReadReportSubmit returned success
//   PumpBusy  - it returned DEVICE_BUSY / NOT_READY
//   PumpBad   - it returned any other failure
//   PumpAbs   - successful submits of the absolute report (reportId 3)
//   PumpKey   - successful submits of the keyboard report
// Published to the registry on every IOCTL, so they can be read at any time.
static volatile LONG g_SubmitIo;
static volatile LONG g_SubmitQ;
static volatile LONG g_PumpCalls;
static volatile LONG g_PumpOk;
static volatile LONG g_PumpBusy;
static volatile LONG g_PumpBad;
static volatile LONG g_PumpAbs;
static volatile LONG g_PumpKey;
// Successes per report ID, indexed by the ID itself (0..7).
static volatile LONG g_Submitted[8];
static PVOID volatile g_VhfHandleLive;

// Last report actually handed to VHF, recorded so a registry read can show
// exactly what crossed the boundary without a debugger attached. Written
// from the pump (PASSIVE, under PumpLock), read from the IOCTL path.
static volatile LONG g_LastSubmitStatus;
static volatile LONG g_LastSubmitId;
static volatile LONG g_LastSubmitLen;
static volatile LONG g_LastSubmitData;

static VOID
VhidInputDiagCleanupCount(
    VOID
    );

static VOID
VhidInputDiagCounters(
    VOID
    );

_Use_decl_annotations_
ULONG
VhidInputReportLength(
    UCHAR ReportId
    )
{
    switch (ReportId) {
    case REPORT_ID_KEYBOARD:
        return KEYBOARD_REPORT_LEN;
    case REPORT_ID_MOUSE_REL:
    case REPORT_ID_MOUSE_ABS:
        return MOUSE_REPORT_LEN;
    case REPORT_ID_MEDIA:
        return MEDIA_REPORT_LEN;
    default:
        return 0;
    }
}


// Take the oldest queued report. Called with PumpLock held. Removes the item
// from the list, so the caller owns it.
static PVHIDINPUT_QUEUED_REPORT
VhidInputPopLocked(
    _In_ PDEVICE_CONTEXT Context
    )
{
    PLIST_ENTRY entry;

    WdfSpinLockAcquire(Context->Queue.Lock);
    if (IsListEmpty(&Context->Queue.List)) {
        WdfSpinLockRelease(Context->Queue.Lock);
        return NULL;
    }
    entry = RemoveHeadList(&Context->Queue.List);
    Context->Queue.Count--;
    WdfSpinLockRelease(Context->Queue.Lock);

    return CONTAINING_RECORD(entry, VHIDINPUT_QUEUED_REPORT, Entry);
}

// HID buffers per report ID, so a busy report ID says nothing about the
// others: their queued reports are not stale and must still be delivered.
// Discard every queued report of RefusedId from anywhere in the queue.
// Called with PumpLock held, never with the spinlock held.
//
// Only DELTA-shaped reports (relative mouse, keyboard, media) are dropped
// here. An absolute mouse report is a whole cursor position rather than a
// change, so a later one supersedes an earlier one and dropping the new one
// can leave the cursor where it is; it is left queued for the next IOCTL to
// pump, and the cap in VhidInputPumpLocked stops that from growing without
// bound.
static VOID
VhidInputDropStaleLocked(
    _In_ PDEVICE_CONTEXT Context,
    _In_ UCHAR RefusedId
    )
{
    LIST_ENTRY dropped;

    InitializeListHead(&dropped);

    WdfSpinLockAcquire(Context->Queue.Lock);
    if (RefusedId != REPORT_ID_MOUSE_ABS) {
        PLIST_ENTRY entry = Context->Queue.List.Flink;
        while (entry != &Context->Queue.List) {
            PLIST_ENTRY next = entry->Flink;
            PVHIDINPUT_QUEUED_REPORT item =
                CONTAINING_RECORD(entry, VHIDINPUT_QUEUED_REPORT, Entry);
            if (item->Report.ReportId == RefusedId) {
                RemoveEntryList(entry);
                InsertTailList(&dropped, entry);
                Context->Queue.Count--;
                Context->Queue.Dropped++;
            }
            entry = next;
        }
    }
    WdfSpinLockRelease(Context->Queue.Lock);

    // Free outside the spinlock: ExFreePoolWithTag at DISPATCH_LEVEL is legal
    // but needlessly long inside a lock the ISR path also takes.
    while (!IsListEmpty(&dropped)) {
        PLIST_ENTRY entry = RemoveHeadList(&dropped);
        ExFreePoolWithTag(CONTAINING_RECORD(entry, VHIDINPUT_QUEUED_REPORT,
                                            Entry), VHIDINPUT_TAG);
    }
}

// Push queued reports to VHF.
//
// This is the only place VhfReadReportSubmit is called. It runs at
// PASSIVE_LEVEL and is serialised by PumpLock, which is what keeps the call
// out of the spinlocked, recursive path seen in the 0x133 dump: no spinlock
// is held across it, and no callback re-enters it. Each submit is handed a
// slot from the rotating pool, not the queue item, because HID may still be
// reading the buffer after the call returns.
//
// Returns STATUS_SUCCESS whenever the queue was drained as far as it could
// go, including when VHF said it was busy: that is a normal back-pressure
// signal, not a failure of the caller's IOCTL. Only NULL handle / delete
// pending are reported as errors.
_Requires_lock_held_(Context->PumpLock)
static NTSTATUS
VhidInputPumpLocked(
    _In_ PDEVICE_CONTEXT Context
    )
{
    PVHIDINPUT_QUEUED_REPORT item;
    NTSTATUS status = STATUS_SUCCESS;
    ULONG submits = 0;

    if (Context->VhfHandle == NULL) {
        return STATUS_DEVICE_NOT_READY;
    }

    InterlockedIncrement(&g_PumpCalls);

    while (submits < VHIDINPUT_PUMP_MAX_PER_CALL) {
        PVHIDINPUT_SUBMIT_SLOT slot;
        UCHAR reportId;
        ULONG length;
        LONG diagData;

        if (ReadNoFence(&Context->VhfCleaned) != 0) {
            // VHF has torn down. Anything left stays queued until the next
            // IOCTL, where a new handle will not exist and it is dropped.
            break;
        }

        item = VhidInputPopLocked(Context);
        if (item == NULL) {
            break;
        }
        reportId = item->Report.ReportId;
        length = VhidInputReportLength(reportId);
        submits++;

        slot = &Context->Slots[Context->NextSlot];
        Context->NextSlot =
            (Context->NextSlot + 1) % VHIDINPUT_SUBMIT_SLOTS;
        // Clear the whole slot first: a reused slot would otherwise carry
        // stale bytes past this report's length, which is harmless to VHF
        // (only `length` bytes are read) but would pollute the diagnostic
        // bytes published for the last submit.
        RtlZeroMemory(slot->Data, sizeof(slot->Data));
        // VHF does not prepend the report ID: reportBuffer must contain the
        // full input report exactly as hidclass will see it, ID byte first.
        // Sending the bare payload left every transfer one byte short and
        // shifted, which hidclass accepted from the pipe but no TLC consumed.
        slot->Data[0] = reportId;
        RtlCopyMemory(&slot->Data[1], item->Report.Data, length);
        slot->Packet.reportBuffer    = slot->Data;
        slot->Packet.reportBufferLen = length + 1;
        slot->Packet.reportId        = reportId;

        // Grab the diagnostic payload before the submit: a ready callback
        // would let VHF read the same buffer asynchronously, and reading back
        // afterwards would race with it.
        //
        // Pack from Data[0], the same place LastSubmitLen's "+ 1" counts from:
        // that byte is the report ID and the payload follows it. Starting at
        // Data[1] dropped the first payload byte, so a one-byte media report
        // packed up as four zero bytes and LastSubmitData always read 0.
        diagData = (LONG)(slot->Data[0] | (slot->Data[1] << 8) |
                          ((ULONG)slot->Data[2] << 16) |
                          ((ULONG)slot->Data[3] << 24));

        status = VhfReadReportSubmit(Context->VhfHandle, &slot->Packet);
        // Record what crossed the boundary: id, full buffer length (ID
        // included), and payload bytes 0..3 packed little-endian. Read back
        // through the registry so a picture of the actual submit needs no
        // debugger.
        InterlockedExchange(&g_LastSubmitId, (LONG)reportId);
        InterlockedExchange(&g_LastSubmitLen, (LONG)(length + 1));
        InterlockedExchange(&g_LastSubmitData, diagData);
        InterlockedExchange(&g_LastSubmitStatus, (LONG)status);
        if (NT_SUCCESS(status)) {
            InterlockedIncrement(&g_PumpOk);
            if (reportId < ARRAYSIZE(g_Submitted)) {
                InterlockedIncrement(&g_Submitted[reportId]);
            }
            if (reportId == REPORT_ID_MOUSE_ABS) {
                InterlockedIncrement(&g_PumpAbs);
            } else if (reportId == REPORT_ID_KEYBOARD) {
                InterlockedIncrement(&g_PumpKey);
            }
        }
        if (status == STATUS_DEVICE_BUSY || status == STATUS_DEVICE_NOT_READY) {
            // VHF's internal buffer is full, or closing. This report was not
            // accepted. Only reports of the same ID that already describe a
            // superseded state go; the others are still owed to the OS.
            // Stop pumping until the next IOCTL; the caller's submit is
            // still a success.
            WdfSpinLockAcquire(Context->Queue.Lock);
            Context->Queue.Dropped++;
            WdfSpinLockRelease(Context->Queue.Lock);
            InterlockedIncrement(&g_PumpBusy);
            DbgPrint("VhidInput: submit 0x%02x busy, 0x%08X\n",
                     reportId, status);
            ExFreePoolWithTag(item, VHIDINPUT_TAG);
            VhidInputDropStaleLocked(Context, reportId);
            status = STATUS_SUCCESS;
            break;
        }
        if (status == STATUS_INVALID_PARAMETER ||
            status == STATUS_INVALID_DEVICE_REQUEST) {
            // The report does not match the descriptor VHF was created with.
            // That is a programming error on the injecting side, not back
            // pressure, so it is worth one line per occurrence.
            WdfSpinLockAcquire(Context->Queue.Lock);
            Context->Queue.Dropped++;
            WdfSpinLockRelease(Context->Queue.Lock);
            InterlockedIncrement(&g_PumpBad);
            DbgPrint("VhidInput: submit 0x%02x rejected, 0x%08X\n",
                     reportId, status);
            status = STATUS_SUCCESS;
        }
        if (!NT_SUCCESS(status)) {
            // Unexpected: count and drop rather than block the whole device
            // on a single bad report.
            WdfSpinLockAcquire(Context->Queue.Lock);
            Context->Queue.Dropped++;
            WdfSpinLockRelease(Context->Queue.Lock);
            InterlockedIncrement(&g_PumpBad);
            DbgPrint("VhidInput: submit 0x%02x refused, 0x%08X\n",
                     reportId, status);
            status = STATUS_SUCCESS;
        }
        // The slot keeps the copy, not the item.
        ExFreePoolWithTag(item, VHIDINPUT_TAG);
    }
    return status;
}

// Take PumpLock and drain. Called from the IOCTL path only.
static NTSTATUS
VhidInputPump(
    _In_ PDEVICE_CONTEXT Context
    )
{
    NTSTATUS status;

    WdfWaitLockAcquire(Context->PumpLock, NULL);
    status = VhidInputPumpLocked(Context);
    WdfWaitLockRelease(Context->PumpLock);
    return status;
}

_Use_decl_annotations_
VOID
VhidInputEvtVhfCleanup(
    PVOID VhfClientContext
    )
{
    PDEVICE_CONTEXT context = (PDEVICE_CONTEXT)VhfClientContext;

    // VHF runs this inside VhfDelete, from HID stack teardown, at
    // DISPATCH_LEVEL or below. It only signals: taking a wait lock here would
    // be illegal above PASSIVE, and would deadlock against
    // VhidInputEvtDeviceCleanup if it held one across VhfDelete.
    //
    // Only interlocked work here: this can run at DISPATCH_LEVEL, where
    // RtlWriteRegistryValue (used to publish the counters) is illegal. The
    // count is published later, from EvtDeviceCleanup at PASSIVE.
    InterlockedExchange(&context->VhfCleaned, 1);
    InterlockedIncrement(&g_VhfCleanupCount);
    DbgPrint("VhidInput: VHF cleanup\n");
}

_Use_decl_annotations_
NTSTATUS
VhidInputEvtReleaseHardware(
    WDFDEVICE Device,
    WDFCMRESLIST ResourcesTranslated
    )
{
    UNREFERENCED_PARAMETER(Device);
    UNREFERENCED_PARAMETER(ResourcesTranslated);
    // The VHF device is not a hardware resource: it is deleted once, from
    // EvtDeviceCleanupCallback. Deleting it here would leave the device
    // without a virtual HID after a stop/start cycle.
    return STATUS_SUCCESS;
}

// WDF runs EvtCleanupCallback for a WDFDEVICE from the device's removal
// path, on the removal thread, so this runs in the PASSIVE stack of that
// thread. That is what makes the synchronous VhfDelete below legal:
// VhfDelete(handle, TRUE) requires PASSIVE and waits for the HID stack's
// remove to complete.
_Use_decl_annotations_
VOID
VhidInputEvtDeviceCleanup(
    WDFOBJECT Object
    )
{
    PDEVICE_CONTEXT context = DeviceGetContext(Object);
    PLIST_ENTRY entry;
    VHFHANDLE handle;

    if (context->PumpLock == NULL) {
        // EvtDeviceAdd failed before the locks existed; nothing was created.
        return;
    }

    // Block new IOCTL work and wait out a pump already inside VHF. Both the
    // lock and the handle are checked: cleanup also runs when EvtDeviceAdd
    // fails half-way through.
    WdfWaitLockAcquire(context->PumpLock, NULL);
    handle = context->HandleForDelete;
    context->HandleForDelete = NULL;
    context->VhfHandle = NULL;
    WdfWaitLockRelease(context->PumpLock);

    // Synchronous delete: Microsoft's guidance for a handle created in
    // EvtDeviceAdd is to call VhfDelete(..., TRUE) from the cleanup callback,
    // at PASSIVE_LEVEL. Doing it here is the only place it happens.
    //
    // This blocks while the HID child PDO is removed. Nothing on that path
    // waits for this cleanup or for PumpLock: VhidInputEvtVhfCleanup only
    // sets a flag, and no IOCTL is in flight (the queue was deleted along
    // with the device).
    if (handle != NULL) {
        DbgPrint("VhidInput: VhfDelete begin\n");
        VhfDelete(handle, TRUE);
        DbgPrint("VhidInput: VhfDelete done\n");
    }

    // Nothing can drain the queue any more; free leftovers.
    while (!IsListEmpty(&context->Queue.List)) {
        entry = RemoveHeadList(&context->Queue.List);
        context->Queue.Count--;
        ExFreePoolWithTag(CONTAINING_RECORD(entry, VHIDINPUT_QUEUED_REPORT,
                                            Entry), VHIDINPUT_TAG);
    }

    // PASSIVE here, so this is the safe place to publish the counters that
    // EvtVhfCleanup (possibly DISPATCH) only bumped.
    VhidInputDiagCleanupCount();
    // Locks are children of the device object.
}

static void
VhidInputDiag(
    _In_ PCWSTR Name,
    _In_ NTSTATUS Status
    )
{
    ULONG value = (ULONG)Status;

    (void)RtlWriteRegistryValue(
        RTL_REGISTRY_ABSOLUTE,
        L"\\Registry\\Machine\\SYSTEM\\CurrentControlSet\\Services\\vhidinput",
        Name, REG_DWORD, &value, sizeof(value));
}

#define DIAG(s, tag) VhidInputDiag(L##tag, (s))

// Publish the delivery counters. Called from the IOCTL path at PASSIVE_LEVEL,
// once per submit, so the registry always reflects the last few injected
// reports. Cheap enough to leave in a test-signed debug build.
static VOID
VhidInputDiagCounters(
    VOID
    )
{
    static const PCWSTR names[] = {
        L"SubmitIo", L"SubmitQ", L"PumpCalls", L"PumpOk",
        L"PumpBusy", L"PumpBad", L"PumpAbs",   L"PumpKey",
    };
    static volatile LONG *const counters[] = {
        &g_SubmitIo, &g_SubmitQ, &g_PumpCalls, &g_PumpOk,
        &g_PumpBusy, &g_PumpBad, &g_PumpAbs,   &g_PumpKey,
    };

    // The handle's live value answers "did VhfStart leave us a usable one"
    // without needing the DeviceAdd-time DIAG.
    {
        ULONG h = (ULONG)(ULONG_PTR)InterlockedCompareExchangePointer(
            (PVOID volatile *)&g_VhfHandleLive, NULL, NULL);
        (void)RtlWriteRegistryValue(
            RTL_REGISTRY_ABSOLUTE,
            L"\\Registry\\Machine\\SYSTEM\\CurrentControlSet\\Services\\vhidinput",
            L"VhfHandleLive", REG_DWORD, &h, sizeof(h));
    }
    ULONG i;

    // Last submit details: id, payload length, first four payload bytes, status.
    {
        static const PCWSTR lastNames[] = {
            L"LastSubmitStatus", L"LastSubmitId",
            L"LastSubmitLen",    L"LastSubmitData",
        };
        static volatile LONG *const lastValues[] = {
            &g_LastSubmitStatus, &g_LastSubmitId,
            &g_LastSubmitLen,    &g_LastSubmitData,
        };
        ULONG j;
        for (j = 0; j < RTL_NUMBER_OF(lastNames); j++) {
            ULONG value = (ULONG)InterlockedCompareExchange(lastValues[j], 0, 0);
            (void)RtlWriteRegistryValue(
                RTL_REGISTRY_ABSOLUTE,
                L"\\Registry\\Machine\\SYSTEM\\CurrentControlSet\\Services\\vhidinput",
                lastNames[j], REG_DWORD, &value, sizeof(value));
        }
    }

    for (i = 0; i < RTL_NUMBER_OF(names); i++) {
        ULONG value = (ULONG)InterlockedCompareExchange(counters[i], 0, 0);
        (void)RtlWriteRegistryValue(
            RTL_REGISTRY_ABSOLUTE,
            L"\\Registry\\Machine\\SYSTEM\\CurrentControlSet\\Services\\vhidinput",
            names[i], REG_DWORD, &value, sizeof(value));
    }
}

static VOID
VhidInputDiagCleanupCount(
    VOID
    )
{
    ULONG value = (ULONG)InterlockedCompareExchange(&g_VhfCleanupCount, 0, 0);

    (void)RtlWriteRegistryValue(
        RTL_REGISTRY_ABSOLUTE,
        L"\\Registry\\Machine\\SYSTEM\\CurrentControlSet\\Services\\vhidinput",
        L"VhfCleanupCount", REG_DWORD, &value, sizeof(value));
}

_Use_decl_annotations_
NTSTATUS
VhidInputEvtPrepareHardware(
    WDFDEVICE Device,
    WDFCMRESLIST ResourcesRaw,
    WDFCMRESLIST ResourcesTranslated
    )
{
    UNREFERENCED_PARAMETER(Device);
    UNREFERENCED_PARAMETER(ResourcesRaw);
    UNREFERENCED_PARAMETER(ResourcesTranslated);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS
VhidInputEvtDeviceAdd(
    WDFDRIVER Driver,
    PWDFDEVICE_INIT DeviceInit
    )
{
    NTSTATUS status;
    WDFDEVICE device;
    WDF_OBJECT_ATTRIBUTES attributes;
    PDEVICE_CONTEXT context;
    DECLARE_CONST_UNICODE_STRING(symlink, VHIDINPUT_SYMLINK_NAME);
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDF_OBJECT_ATTRIBUTES lockAttributes;
    WDF_PNPPOWER_EVENT_CALLBACKS pnpCallbacks;
    VHF_CONFIG vhfConfig;

    UNREFERENCED_PARAMETER(Driver);

    // This is a PnP root-enumerated device; assigning an explicit device
    // name is rejected for PnP FDOs, so only expose a DOS symlink and a
    // device interface, both created after WdfDeviceCreate.

    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, DEVICE_CONTEXT);
    attributes.EvtCleanupCallback = VhidInputEvtDeviceCleanup;

    // Needed so VhidInputEvtReleaseHardware runs on removal, and so the
    // virtual HID device is created with the device rather than at the first
    // IOCTL. Remove/stop only calls the PnP callbacks, not EvtCleanup.
    WDF_PNPPOWER_EVENT_CALLBACKS_INIT(&pnpCallbacks);
    pnpCallbacks.EvtDeviceReleaseHardware = VhidInputEvtReleaseHardware;
    WdfDeviceInitSetPnpPowerEventCallbacks(DeviceInit, &pnpCallbacks);

    status = WdfDeviceCreate(&DeviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        DIAG(status, "Create");
        return status;
    }
    context = DeviceGetContext(device);
    // WDF zeroes the context; only the list head and locks need explicit work.
    InitializeListHead(&context->Queue.List);

    WDF_OBJECT_ATTRIBUTES_INIT(&lockAttributes);
    lockAttributes.ParentObject = device;
    status = WdfSpinLockCreate(&lockAttributes, &context->Queue.Lock);
    if (!NT_SUCCESS(status)) {
        DIAG(status, "SpinLock");
        return status;
    }
    status = WdfWaitLockCreate(&lockAttributes, &context->PumpLock);
    if (!NT_SUCCESS(status)) {
        DIAG(status, "PumpLock");
        return status;
    }

    // User mode talks to the driver through one DeviceIoControl queue.
    // Parallel dispatch lets several callers inject at once; the pump
    // serialises the VHF calls itself.
    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig,
                                           WdfIoQueueDispatchParallel);
    queueConfig.EvtIoDeviceControl = VhidInputEvtIoDeviceControl;
    queueConfig.PowerManaged = WdfFalse;
    // Default is WdfUseDefault, i.e. one request at a time. Injecting a burst
    // through one file handle must not serialise on the round trip, so
    // allow several requests in flight; the pump serialises VHF itself.
    queueConfig.Settings.Parallel.NumberOfPresentedRequests = 64;

    status = WdfIoQueueCreate(device, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES,
                              WDF_NO_HANDLE);
    if (!NT_SUCCESS(status)) {
        DIAG(status, "Queue");
        return status;
    }

    // The DOS symlink is a convenience for `\\.\VhidInput`; the device
    // interface below is the proper PnP way to find the device. A second
    // instance of this driver (a stale root node left behind by a previous
    // install) would claim the same DOS name and fail with
    // STATUS_INVALID_DEVICE_REQUEST here, so that must not be fatal: the
    // device still works through its interface. Failure is recorded only.
    status = WdfDeviceCreateSymbolicLink(device, &symlink);
    if (!NT_SUCCESS(status)) {
        DbgPrint("VhidInput: CreateSymbolicLink failed 0x%08X\n", status);
        DIAG(status, "Symlink");
    }

    status = WdfDeviceCreateDeviceInterface(device,
                                            &g_VhidInputInterfaceGuid,
                                            NULL);
    if (!NT_SUCCESS(status)) {
        DbgPrint("VhidInput: CreateDeviceInterface failed 0x%08X\n", status);
        DIAG(status, "Iface");
        return status;
    }

    // Per the VHF documentation, create and start the virtual HID device
    // from EvtDeviceAdd after WdfDeviceCreate.
    VHF_CONFIG_INIT(&vhfConfig,
                    WdfDeviceWdmGetDeviceObject(device),
                    (USHORT)g_ReportDescriptorSize,
                    (PUCHAR)g_ReportDescriptor);
    vhfConfig.VhfClientContext              = context;
    // Deliberately no EvtVhfReadyForNextReadReport: VHF then owns the
    // buffering ("default buffering policy"), and the documentation allows
    // the transfer buffer to be reused as soon as VhfReadReportSubmit
    // returns. With a ready callback registered, the buffer must stay alive
    // until the *next* callback, which would force a pending slot and a
    // state machine for no benefit here.
    vhfConfig.VendorID                      = VHIDINPUT_VENDOR_ID;
    vhfConfig.ProductID                     = VHIDINPUT_PRODUCT_ID;
    vhfConfig.VersionNumber                 = VHIDINPUT_VERSION_NUMBER;
    // VhfClientContext is what this callback gets back, so it is context.
    vhfConfig.EvtVhfCleanup                 = VhidInputEvtVhfCleanup;

    DIAG((NTSTATUS)vhfConfig.Size, "VhfConfigSize");
    DIAG((NTSTATUS)(ULONG)(ULONG_PTR)vhfConfig.DeviceObject, "VhfDeviceObj");

    status = VhfCreate(&vhfConfig, &context->VhfHandle);
    if (!NT_SUCCESS(status)) {
        DbgPrint("VhidInput: VhfCreate failed 0x%08X\n", status);
        DIAG(status, "VhfCreate");
        return status;
    }
    // VhfHandle stops new submits during teardown; HandleForDelete is what
    // VhfDelete is actually given, so that the two steps stay separate.
    context->HandleForDelete = context->VhfHandle;

    status = VhfStart(context->VhfHandle);
    // Always publish the VhfStart result: a device whose VHF was created but
    // never started still enumerates its HID children, yet delivers nothing.
    // That is indistinguishable from a healthy device unless this is recorded.
    DIAG(status, "VhfStart");
    InterlockedExchangePointer((PVOID volatile *)&g_VhfHandleLive,
                               context->VhfHandle);
    if (!NT_SUCCESS(status)) {
        DbgPrint("VhidInput: VhfStart failed 0x%08X\n", status);
        // A created handle has to be deleted exactly once, and cleanup is the
        // single place that does it. Return success anyway: EvtDeviceCleanup
        // only runs if the device object was created, and the driver would
        // otherwise leave a device behind that can never deliver anything.
        status = STATUS_SUCCESS;
    }

    return STATUS_SUCCESS;
}

// Accept one input report from user mode, then pump. Runs at PASSIVE_LEVEL.
static NTSTATUS
VhidInputSubmitReport(
    _In_ PDEVICE_CONTEXT Context,
    _In_ PVHIDINPUT_REPORT Report,
    _Out_ ULONG *PendingAfter
    )
{
    PVHIDINPUT_QUEUED_REPORT item;
    ULONG length = VhidInputReportLength(Report->ReportId);
    NTSTATUS status;
    BOOLEAN queued = FALSE;

    InterlockedIncrement(&g_SubmitIo);

    // Slots are sized by VHIDINPUT_MAX_REPORT_DATA; a descriptor change that
    // grew a report past it would otherwise overflow the copy in the pump.
    // This is the check that enforces it, since ASSERT is compiled out in
    // release builds (mk.bat does not define DBG).
    if (length == 0 || length > VHIDINPUT_MAX_REPORT_DATA) {
        DbgPrint("VhidInput: bad report id=0x%02x len=%lu\n",
                 (ULONG)Report->ReportId, length);
        return STATUS_INVALID_PARAMETER;
    }

    item = (PVHIDINPUT_QUEUED_REPORT)ExAllocatePool2(
        POOL_FLAG_NON_PAGED, sizeof(*item), VHIDINPUT_TAG);
    if (item == NULL) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    item->Report = *Report;

    WdfSpinLockAcquire(Context->Queue.Lock);
    if (Context->VhfHandle == NULL || ReadNoFence(&Context->VhfCleaned) != 0) {
        // Device is being removed, or VHF has already gone away; there is
        // nobody left to deliver to.
        Context->Queue.DroppedBacklog++;
        status = STATUS_DELETE_PENDING;
    } else if (Context->Queue.Count >= VHIDINPUT_QUEUE_CAPACITY) {
        Context->Queue.DroppedBacklog++;
        status = STATUS_INSUFFICIENT_RESOURCES;
    } else {
        InsertTailList(&Context->Queue.List, &item->Entry);
        Context->Queue.Count++;
        InterlockedIncrement(&g_SubmitQ);
        status = STATUS_SUCCESS;
        queued = TRUE;
    }
    WdfSpinLockRelease(Context->Queue.Lock);

    if (!queued) {
        ExFreePoolWithTag(item, VHIDINPUT_TAG);
        return status;
    }

    status = VhidInputPump(Context);
    if (!NT_SUCCESS(status)) {
        // The report is still in the queue and will be retried; a pump
        // failure must not turn into a lost input report.
        DbgPrint("VhidInput: pump failed 0x%08X\n", status);
    }

    WdfSpinLockAcquire(Context->Queue.Lock);
    *PendingAfter = Context->Queue.Count;
    WdfSpinLockRelease(Context->Queue.Lock);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
VOID
VhidInputEvtIoDeviceControl(
    WDFQUEUE Queue,
    WDFREQUEST Request,
    size_t OutputBufferLength,
    size_t InputBufferLength,
    ULONG IoControlCode
    )
{
    PDEVICE_CONTEXT context = DeviceGetContext(WdfIoQueueGetDevice(Queue));
    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;
    ULONG bytesReturned = 0;

    UNREFERENCED_PARAMETER(OutputBufferLength);

    switch (IoControlCode) {
    case VHIDINPUT_IOCTL_SUBMIT_REPORT: {
        PVHIDINPUT_REPORT report;
        ULONG pending = 0;

        PVOID out = NULL;

        if (InputBufferLength != sizeof(VHIDINPUT_REPORT)) {
            status = STATUS_INVALID_BUFFER_SIZE;
            break;
        }
        status = WdfRequestRetrieveInputBuffer(Request,
                                               sizeof(VHIDINPUT_REPORT),
                                               (PVOID *)&report, NULL);
        if (!NT_SUCCESS(status)) {
            break;
        }
        {
            ULONG _l = VhidInputReportLength(report->ReportId);
            DbgPrint("VhidInput: ioctl id=0x%02x len=%lu\n",
                     (ULONG)report->ReportId, _l);
        }
        status = VhidInputSubmitReport(context, report, &pending);
        DbgPrint("VhidInput: ioctl done 0x%08X\n", status);
        VhidInputDiagCounters();
        if (NT_SUCCESS(status)) {
            status = WdfRequestRetrieveOutputBuffer(Request, sizeof(ULONG),
                                                    (PVOID *)&out, NULL);
            if (NT_SUCCESS(status)) {
                *(ULONG *)out = pending;
                bytesReturned = sizeof(ULONG);
                status = STATUS_SUCCESS;
            }
        }
        break;
    }

    case VHIDINPUT_IOCTL_GET_STATUS: {
        PVHIDINPUT_STATUS out;

        status = WdfRequestRetrieveOutputBuffer(Request,
                                                sizeof(VHIDINPUT_STATUS),
                                                (PVOID *)&out, NULL);
        if (!NT_SUCCESS(status)) {
            break;
        }
        WdfSpinLockAcquire(context->Queue.Lock);
        out->ProtocolVersion = VHIDINPUT_PROTOCOL_VERSION;
        out->PendingReports  = context->Queue.Count;
        out->DroppedReports  = context->Queue.Dropped +
                               context->Queue.DroppedBacklog;
        out->MaxPending      = VHIDINPUT_QUEUE_CAPACITY;
        for (ULONG i = 0; i < 8; i++) {
            out->Submitted[i] = (ULONG)ReadNoFence(&g_Submitted[i]);
        }
        WdfSpinLockRelease(context->Queue.Lock);
        bytesReturned = sizeof(VHIDINPUT_STATUS);
        status = STATUS_SUCCESS;
        break;
    }

    default:
        status = STATUS_INVALID_DEVICE_REQUEST;
        break;
    }

    WdfRequestCompleteWithInformation(Request, status, bytesReturned);
}

_Use_decl_annotations_
NTSTATUS
DriverEntry(
    PDRIVER_OBJECT DriverObject,
    PUNICODE_STRING RegistryPath
    )
{
    WDF_DRIVER_CONFIG config;
    NTSTATUS status;

    UNREFERENCED_PARAMETER(RegistryPath);

    WDF_DRIVER_CONFIG_INIT(&config, VhidInputEvtDeviceAdd);

    DIAG(STATUS_SUCCESS, "Entered");

    status = WdfDriverCreate(DriverObject, RegistryPath,
                             WDF_NO_OBJECT_ATTRIBUTES, &config,
                             WDF_NO_HANDLE);
    DIAG(status, "WdfDriverCreate");
    if (!NT_SUCCESS(status)) {
        DbgPrint("VhidInput: WdfDriverCreate failed 0x%08X\n", status);
    }
    return status;
}
