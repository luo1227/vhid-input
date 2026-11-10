#include "vhidinput.h"

// The interface GUID is defined here and only declared in the header, so
// the symbol exists exactly once across the driver's translation units.
const GUID g_VhidInputInterfaceGuid = {
    0x7F3C2A51, 0x8B4E, 0x4C7D,
    0x9F, 0x2A, 0x1D, 0x6E, 0x5B, 0x3C, 0x4A, 0x90 };

// HID report descriptor for a composite keyboard + relative mouse +
// absolute mouse + media-key device, all in one top-level collection.
//
// Layout (report ID, payload bytes):
//   0x01 Keyboard   : modifiers, reserved, 6 keycodes                -> 8 bytes
//   0x02 Mouse (rel): buttons, dx int16, dy int16, wheel int8        -> 6 bytes
//   0x03 Mouse (abs): buttons, x uint16, y uint16, wheel int8        -> 6 bytes
//   0x04 Media keys : 6 bit-flags + 2 pad                       -> 1 byte
//
// The absolute collection spans the full 0..32767 logical range, which
// Windows maps onto the whole virtual desktop. The user-mode side
// normalises pixel coordinates into that range; see user/src/lib.rs.
//
// Windows arms the HID stack on boot, so a virtual HID device cannot carry
// the Boot Interface subclass, or the device would be rejected.

const UCHAR g_ReportDescriptor[] = {

    // ---- Keyboard (Usage Page 0x01, Usage 0x06) ----
    0x05, 0x01,        // Usage Page (Generic Desktop)
    0x09, 0x06,        // Usage (Keyboard)
    0xA1, 0x01,        // Collection (Application)
    0x85, 0x01,        //   Report ID (1)
    0x05, 0x07,        //   Usage Page (Keyboard/Keypad)
    0x19, 0xE0,        //   Usage Minimum (Left Control)
    0x29, 0xE7,        //   Usage Maximum (Right GUI)
    0x15, 0x00,        //   Logical Minimum (0)
    0x25, 0x01,        //   Logical Maximum (1)
    0x75, 0x01,        //   Report Size (1)
    0x95, 0x08,        //   Report Count (8)
    0x81, 0x02,        //   Input (Data,Var,Abs)     -> modifiers
    0x95, 0x01,        //   Report Count (1)
    0x75, 0x08,        //   Report Size (8)
    0x81, 0x03,        //   Input (Const,Var,Abs)    -> reserved
    0x95, 0x06,        //   Report Count (6)
    0x75, 0x08,        //   Report Size (8)
    0x15, 0x00,        //   Logical Minimum (0)
    0x25, 0x65,        //   Logical Maximum (101)
    0x05, 0x07,        //   Usage Page (Keyboard/Keypad)
    0x19, 0x00,        //   Usage Minimum (0)
    0x29, 0x65,        //   Usage Maximum (101)
    0x81, 0x00,        //   Input (Data,Array)       -> key slots
    0xC0,              // End Collection

    // ---- Relative mouse (Usage Page 0x01, Usage 0x02) ----
    0x05, 0x01,        // Usage Page (Generic Desktop)
    0x09, 0x02,        // Usage (Mouse)
    0xA1, 0x01,        // Collection (Application)
    0x85, 0x02,        //   Report ID (2)
    0x09, 0x01,        //   Usage (Pointer)
    0xA1, 0x00,        //   Collection (Physical)
    0x05, 0x09,        //     Usage Page (Button)
    0x19, 0x01,        //     Usage Minimum (1)
    0x29, 0x05,        //     Usage Maximum (5)     -> left/right/middle/side1/side2
    0x15, 0x00,        //     Logical Minimum (0)
    0x25, 0x01,        //     Logical Maximum (1)
    0x75, 0x01,        //     Report Size (1)
    0x95, 0x05,        //     Report Count (5)
    0x81, 0x02,        //     Input (Data,Var,Abs)
    0x95, 0x03,        //     Report Count (3)
    0x81, 0x03,        //     Input (Const)         -> pad
    0x05, 0x01,        //     Usage Page (Generic Desktop)
    0x09, 0x30,        //     Usage (X)
    0x09, 0x31,        //     Usage (Y)
    0x16, 0x01, 0x80,  //     Logical Minimum (-32767)
    0x26, 0xFF, 0x7F,  //     Logical Maximum (32767)
    0x75, 0x10,        //     Report Size (16)
    0x95, 0x02,        //     Report Count (2)
    0x81, 0x06,        //     Input (Data,Var,Rel)
    0x09, 0x38,        //     Usage (Wheel)
    0x15, 0x81,        //     Logical Minimum (-127)
    0x25, 0x7F,        //     Logical Maximum (127)
    0x75, 0x08,        //     Report Size (8)
    0x95, 0x01,        //     Report Count (1)
    0x81, 0x06,        //     Input (Data,Var,Rel)
    0xC0,              //   End Collection (Physical)
    0xC0,              // End Collection

    // ---- Absolute mouse (Usage Page 0x01, Usage 0x02) ----
    0x05, 0x01,        // Usage Page (Generic Desktop)
    0x09, 0x02,        // Usage (Mouse)
    0xA1, 0x01,        // Collection (Application)
    0x85, 0x03,        //   Report ID (3)
    0x09, 0x01,        //   Usage (Pointer)
    0xA1, 0x00,        //   Collection (Physical)
    0x05, 0x09,        //     Usage Page (Button)
    0x19, 0x01,        //     Usage Minimum (1)
    0x29, 0x05,        //     Usage Maximum (5)
    0x15, 0x00,        //     Logical Minimum (0)
    0x25, 0x01,        //     Logical Maximum (1)
    0x75, 0x01,        //     Report Size (1)
    0x95, 0x05,        //     Report Count (5)
    0x81, 0x02,        //     Input (Data,Var,Abs)
    0x95, 0x03,        //     Report Count (3)
    0x81, 0x03,        //     Input (Const)         -> pad
    0x05, 0x01,        //     Usage Page (Generic Desktop)
    0x09, 0x30,        //     Usage (X)
    0x09, 0x31,        //     Usage (Y)
    0x16, 0x00, 0x00,  //     Logical Minimum (0)
    0x26, 0xFF, 0x7F,  //     Logical Maximum (32767)
    0x75, 0x10,        //     Report Size (16)
    0x95, 0x02,        //     Report Count (2)
    0x81, 0x02,        //     Input (Data,Var,Abs)
    // Leave physical range unset: HID leaves Absolute mode when Physical
    // Max exceeds Logical Max, and Windows then stops scaling to the
    // desktop.
    0x09, 0x38,        //     Usage (Wheel)
    0x15, 0x81,        //     Logical Minimum (-127)
    0x25, 0x7F,        //     Logical Maximum (127)
    0x75, 0x08,        //     Report Size (8)
    0x95, 0x01,        //     Report Count (1)
    0x81, 0x06,        //     Input (Data,Var,Rel)
    0xC0,              //   End Collection (Physical)
    0xC0,              // End Collection

    // ---- Media keys (Usage Page 0x0C, Consumer) ----
    0x05, 0x0C,        // Usage Page (Consumer)
    0x09, 0x01,        // Usage (Consumer Control)
    0xA1, 0x01,        // Collection (Application)
    0x85, 0x04,        //   Report ID (4)
    0x15, 0x00,        //   Logical Minimum (0)
    0x25, 0x01,        //   Logical Maximum (1)
    0x75, 0x01,        //   Report Size (1)
    // Volume Up / Volume Down / Mute / Scan Next / Scan Prev / Play-Pause
    0x09, 0xE9,        //   Usage (Volume Increment)
    0x09, 0xEA,        //   Usage (Volume Decrement)
    0x09, 0xE2,        //   Usage (Mute)
    0x09, 0xB5,        //   Usage (Scan Next Track)
    0x09, 0xB6,        //   Usage (Scan Previous Track)
    0x09, 0xCD,        //   Usage (Play/Pause)
    0x95, 0x06,        //   Report Count (6)
    0x81, 0x02,        //   Input (Data,Var,Abs)
    0x95, 0x02,        //   Report Count (2)
    0x81, 0x03,        //   Input (Const)          -> pad to whole byte
    0xC0,              // End Collection
};

const ULONG g_ReportDescriptorSize = sizeof(g_ReportDescriptor);
