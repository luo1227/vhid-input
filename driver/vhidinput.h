#pragma once

#include "wdfpre.h"

// Report IDs. Matches the descriptor in vhidinput_report.c
#define REPORT_ID_KEYBOARD   0x01
#define REPORT_ID_MOUSE_REL  0x02
#define REPORT_ID_MOUSE_ABS  0x03
#define REPORT_ID_MEDIA      0x04

// Input report lengths as declared by the descriptor (excluding report ID).
#define KEYBOARD_REPORT_LEN  8   // modifiers + reserved + 6 keys
#define MOUSE_REPORT_LEN     6   // buttons + x(2) + y(2) + wheel
#define MEDIA_REPORT_LEN     1   // 6 flags + 2 pad

// Longest payload any report carries. The largest item a submit is ever
// handed, and therefore the copy buffer kept for VHF.
#define VHIDINPUT_MAX_REPORT_DATA 8

#pragma pack(push, 1)
typedef struct _VHIDINPUT_REPORT {
    UCHAR ReportId;
    UCHAR Data[64];
} VHIDINPUT_REPORT, *PVHIDINPUT_REPORT;
#pragma pack(pop)

#define VHIDINPUT_DEVICE_NAME   L"\\Device\\VhidInput"
#define VHIDINPUT_SYMLINK_NAME  L"\\DosDevices\\VhidInput"

#define VHIDINPUT_VENDOR_ID      0x1209
#define VHIDINPUT_PRODUCT_ID     0x0001
#define VHIDINPUT_VERSION_NUMBER 0x0100

#define VHIDINPUT_TAG 0x41694956  // 'VIA'

// IOCTL interface
#define VHIDINPUT_DEVICE_TYPE  0x9F00
#define VHIDINPUT_IOCTL_SUBMIT_REPORT \
    CTL_CODE(VHIDINPUT_DEVICE_TYPE, 0x801, METHOD_BUFFERED, \
             FILE_WRITE_DATA | FILE_READ_DATA)
#define VHIDINPUT_IOCTL_GET_STATUS \
    CTL_CODE(VHIDINPUT_DEVICE_TYPE, 0x802, METHOD_BUFFERED, FILE_READ_DATA)

#pragma pack(push, 1)
typedef struct _VHIDINPUT_STATUS {
    ULONG ProtocolVersion;
    ULONG PendingReports;
    ULONG DroppedReports;
    ULONG MaxPending;
    // Per-report-ID totals of what actually got through VHF, so a caller can
    // tell "went into the driver" from "reached the HID stack".
    ULONG Submitted[8];
} VHIDINPUT_STATUS, *PVHIDINPUT_STATUS;
#pragma pack(pop)

#define VHIDINPUT_PROTOCOL_VERSION 3
#define VHIDINPUT_QUEUE_CAPACITY   256

// Reports one IOCTL is allowed to feed to VHF. Bounds the time spent inside
// VhfReadReportSubmit on a single caller's thread.
#define VHIDINPUT_PUMP_MAX_PER_CALL 16

// {7F3C2A51-8B4E-4C7D-9F2A-1D6E5B3C4A90}
// Defined in vhidinput_report.c; the user-mode crate hard-codes the same
// value in its open_device fallback.
extern const GUID g_VhidInputInterfaceGuid;

// The pump builds the HID_XFER_PACKET from a submit slot rather than from
// the queue item, so an item only carries the raw report.
typedef struct _VHIDINPUT_QUEUED_REPORT {
    LIST_ENTRY Entry;
    VHIDINPUT_REPORT Report;
} VHIDINPUT_QUEUED_REPORT, *PVHIDINPUT_QUEUED_REPORT;

// Staging buffers for VhfReadReportSubmit.
//
// With no EvtVhfReadyForNextReadReport registered, VHF uses its default
// buffering policy and the documentation allows the transfer buffer to be
// reused as soon as VhfReadReportSubmit returns. A single slot would
// therefore be correct. The pool exists so that this stays true if a ready
// callback is ever added: then the buffer must live until the next callback,
// and one slot cannot serve 16 submits in one pump. As long as a pump is the
// only producer and each slot is untouched for VHIDINPUT_SUBMIT_SLOTS
// submits before reuse, the layouts below do not depend on that promise.
#define VHIDINPUT_SUBMIT_SLOTS 32

// One extra byte beyond the maximum payload: the submit buffer carries the
// report ID as its first byte (see VhidInputPumpLocked).
typedef struct _VHIDINPUT_SUBMIT_SLOT {
    UCHAR Data[VHIDINPUT_MAX_REPORT_DATA + 1];
    HID_XFER_PACKET Packet;
} VHIDINPUT_SUBMIT_SLOT, *PVHIDINPUT_SUBMIT_SLOT;

// FIFO of queued reports. Kept as its own object so the spinlock only guards
// a tiny structure and is never held for more than list surgery.
typedef struct _VHIDINPUT_QUEUE {
    WDFSPINLOCK Lock;
    LIST_ENTRY List;
    ULONG Count;
    ULONG Dropped;
    ULONG DroppedBacklog;   // entries discarded because the queue was full
} VHIDINPUT_QUEUE, *PVHIDINPUT_QUEUE;

typedef struct _DEVICE_CONTEXT {
    // The one VHF handle. VhfHandle is cleared under PumpLock at teardown to
    // stop new submits; HandleForDelete keeps the value so VhfDelete runs
    // exactly once even if both the PnP and the cleanup path try.
    VHFHANDLE VhfHandle;
    VHFHANDLE HandleForDelete;

    // Set by EvtVhfCleanup once VHF will not take another report. Read from
    // the submit path.
    volatile LONG VhfCleaned;

    // The pump lock is a passive-level wait lock, so it may be held while
    // calling VhfReadReportSubmit. It is the only thing serialising those
    // calls: they must never happen with a spinlock held, which is what
    // produced the 0x133 dump. It also guards NextSlot, because only the
    // pump touches it.
    WDFWAITLOCK PumpLock;

    ULONG NextSlot;
    VHIDINPUT_SUBMIT_SLOT Slots[VHIDINPUT_SUBMIT_SLOTS];

    VHIDINPUT_QUEUE Queue;
} DEVICE_CONTEXT, *PDEVICE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(DEVICE_CONTEXT, DeviceGetContext)

ULONG VhidInputReportLength(_In_ UCHAR ReportId);

VOID
VhidInputEvtVhfCleanup(_In_ PVOID VhfClientContext);

NTSTATUS
VhidInputEvtDeviceAdd(_In_ WDFDRIVER Driver, _In_ PWDFDEVICE_INIT DeviceInit);

VOID
VhidInputEvtDeviceCleanup(_In_ WDFOBJECT Object);

NTSTATUS
VhidInputEvtPrepareHardware(_In_ WDFDEVICE Device,
                            _In_ WDFCMRESLIST ResourcesRaw,
                            _In_ WDFCMRESLIST ResourcesTranslated);

NTSTATUS
VhidInputEvtReleaseHardware(_In_ WDFDEVICE Device,
                            _In_ WDFCMRESLIST ResourcesTranslated);

VOID
VhidInputEvtIoDeviceControl(_In_ WDFQUEUE Queue,
                            _In_ WDFREQUEST Request,
                            _In_ size_t OutputBufferLength,
                            _In_ size_t InputBufferLength,
                            _In_ ULONG IoControlCode);

extern const UCHAR g_ReportDescriptor[];
extern const ULONG g_ReportDescriptorSize;

DRIVER_INITIALIZE DriverEntry;
