//! PlayStation pads (DualSense, DualSense Edge, DualShock 4) over HID, USB or
//! Bluetooth. They are not XInput devices, so XInput never sees them; a
//! thread reads the pad's input reports and keeps its newest state, which the
//! host source hands out in the first pad slot XInput leaves empty.
//!
//! Device discovery and the read thread follow belobug's 34a0823 ("Input:
//! read DualSense pads over HID when XInput has none", by Kam471); the reports
//! are decoded here by fixed offsets instead, because a DualSense switched to
//! its full Bluetooth report (0x31) declares those fields only as vendor
//! bytes, and the DualShock 4 is covered by the same table.
//!
//! shortcut: touchpad, motion, lightbar and rumble are not used; the PS
//! button has no pad bit.

use std::sync::{Mutex, Once, PoisonError};
use std::time::Duration;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    GUID_DEVINTERFACE_HID, HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData,
    HidD_GetAttributes, HidD_GetFeature, HidD_GetPreparsedData, HidP_GetCaps,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
};

use super::{
    A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, PadState, RIGHT, RIGHT_SHOULDER,
    RIGHT_STICK, UP, VIEW, X, Y,
};

const SONY: u16 = 0x054C;
const DUALSENSE: [u16; 2] = [0x0CE6, 0x0DF2]; // and the Edge
const DUALSHOCK4: [u16; 3] = [0x05C4, 0x09CC, 0x0BA0]; // v1, v2, USB dongle
/// A DualSense's input report length over USB; over Bluetooth it is 78.
const DUALSENSE_USB_LEN: usize = 64;

static STATE: Mutex<Option<PadState>> = Mutex::new(None);
static START: Once = Once::new();

/// The pad's newest state, `None` while no pad is connected. The first call
/// starts the reader (`VICTORY_PS_PAD=0` keeps it off).
pub(super) fn latest() -> Option<PadState> {
    START.call_once(|| {
        if std::env::var("VICTORY_PS_PAD").is_ok_and(|v| v == "0") {
            tracing::info!("PlayStation pad reader off (VICTORY_PS_PAD=0)");
            return;
        }
        let _ = std::thread::Builder::new()
            .name("ps-pad".into())
            .spawn(reader);
    });
    *STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How the pad lays out its report 0x01.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Layout {
    /// DualShock 4 (both transports) and a DualSense's short Bluetooth report:
    /// sticks, buttons, triggers.
    Short,
    /// A DualSense over USB: sticks, triggers, a counter, buttons.
    Full,
}

struct Pad {
    handle: HANDLE,
    layout: Layout,
    input_len: usize,
}

impl Drop for Pad {
    fn drop(&mut self) {
        // SAFETY: ours, from CreateFileW.
        unsafe { CloseHandle(self.handle) };
    }
}

fn reader() {
    let mut searching_logged = false;
    loop {
        match open() {
            Some(pad) => {
                read(&pad);
                *STATE.lock().unwrap_or_else(PoisonError::into_inner) = None;
                tracing::info!("PlayStation pad disconnected");
                searching_logged = false;
            }
            None if !searching_logged => {
                tracing::info!("no PlayStation pad found over HID; looking again every 2 s");
                searching_logged = true;
            }
            None => {}
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Reads reports until the pad goes away.
fn read(pad: &Pad) {
    let mut buf = vec![0u8; pad.input_len.max(DUALSENSE_USB_LEN)];
    let mut first = true;
    loop {
        let mut len = 0u32;
        // SAFETY: a live handle and a buffer at least the report length.
        let ok = unsafe {
            ReadFile(
                pad.handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut len,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || len == 0 {
            return;
        }
        let Some(state) = decode(pad.layout, &buf[..len as usize]) else {
            continue;
        };
        if first {
            tracing::info!(
                report = format_args!("{:#04x}", buf[0]),
                "PlayStation pad reporting"
            );
            first = false;
        }
        *STATE.lock().unwrap_or_else(PoisonError::into_inner) = Some(state);
    }
}

/// Opens the first PlayStation pad among the present HID interfaces.
fn open() -> Option<Pad> {
    for path in hid_paths() {
        // SAFETY: a NUL-terminated path from SetupAPI, opened shared.
        let open = |access| unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        // Writing is needed only for the feature request below.
        let mut handle = open(GENERIC_READ | GENERIC_WRITE);
        if handle == INVALID_HANDLE_VALUE {
            handle = open(GENERIC_READ);
        }
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }
        let mut attributes = HIDD_ATTRIBUTES {
            Size: size_of::<HIDD_ATTRIBUTES>() as u32,
            VendorID: 0,
            ProductID: 0,
            VersionNumber: 0,
        };
        // SAFETY: a live handle and room for the attributes.
        let known = unsafe { HidD_GetAttributes(handle, &mut attributes) };
        let product = attributes.ProductID;
        let dualsense = DUALSENSE.contains(&product);
        if !known || attributes.VendorID != SONY || !(dualsense || DUALSHOCK4.contains(&product)) {
            // SAFETY: ours, not kept.
            unsafe { CloseHandle(handle) };
            continue;
        }
        let (input_len, feature_len) = lengths(handle).unwrap_or((DUALSENSE_USB_LEN, 0));
        if input_len != DUALSENSE_USB_LEN {
            // Over Bluetooth the pad sends short reports, and only when
            // something changes, until its calibration is read (feature 0x05,
            // DualShock 4: 0x02); then full reports (0x31 / 0x11) stream.
            let mut feature = vec![0u8; feature_len.max(64)];
            feature[0] = if dualsense { 0x05 } else { 0x02 };
            // SAFETY: a live handle and a buffer of the feature length.
            let full = unsafe {
                HidD_GetFeature(
                    handle,
                    feature.as_mut_ptr().cast(),
                    feature_len.max(64) as u32,
                )
            };
            tracing::info!(full, "PlayStation pad: asked for full Bluetooth reports");
        }
        let layout = if dualsense && input_len == DUALSENSE_USB_LEN {
            Layout::Full
        } else {
            Layout::Short
        };
        tracing::info!(
            pad = if dualsense {
                "DualSense"
            } else {
                "DualShock 4"
            },
            product = format_args!("{product:#06x}"),
            transport = if input_len == DUALSENSE_USB_LEN {
                "USB"
            } else {
                "Bluetooth"
            },
            report = input_len,
            "PlayStation pad connected over HID"
        );
        return Some(Pad {
            handle,
            layout,
            input_len,
        });
    }
    None
}

/// The device's input and feature report lengths, report ID included.
fn lengths(handle: HANDLE) -> Option<(usize, usize)> {
    let mut preparsed = 0;
    // SAFETY: a live handle and room for the pointer.
    if !unsafe { HidD_GetPreparsedData(handle, &mut preparsed) } {
        return None;
    }
    // SAFETY: all zeroes is a valid HIDP_CAPS.
    let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
    // SAFETY: the preparsed data is ours until freed just below.
    let status = unsafe { HidP_GetCaps(preparsed, &mut caps) };
    // SAFETY: from HidD_GetPreparsedData.
    unsafe { HidD_FreePreparsedData(preparsed) };
    (status == HIDP_STATUS_SUCCESS && caps.InputReportByteLength > 0).then_some((
        usize::from(caps.InputReportByteLength),
        usize::from(caps.FeatureReportByteLength),
    ))
}

/// Every present HID interface's path, NUL-terminated.
fn hid_paths() -> Vec<Vec<u16>> {
    let mut paths = Vec::new();
    // SAFETY: the HID interface class; no enumerator, no window.
    let set = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_HID,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == -1 {
        return paths;
    }
    for index in 0.. {
        // SAFETY: all zeroes is valid; cbSize set below.
        let mut interface: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
        interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        // SAFETY: the set from above and room for one interface.
        if unsafe {
            SetupDiEnumDeviceInterfaces(
                set,
                std::ptr::null(),
                &GUID_DEVINTERFACE_HID,
                index,
                &mut interface,
            )
        } == 0
        {
            break;
        }
        let mut required = 0u32;
        // SAFETY: asks only for the size the detail needs.
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                std::ptr::null_mut(),
                0,
                &mut required,
                std::ptr::null_mut(),
            )
        };
        if (required as usize) < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() {
            continue;
        }
        // u32-aligned room for the detail: cbSize, then the UTF-16 path.
        let mut buffer = vec![0u32; (required as usize).div_ceil(4)];
        let detail = buffer
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // SAFETY: the buffer is as large as the API asked; cbSize is the
        // fixed part's size, as the API wants.
        let ok = unsafe {
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                detail,
                required,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            continue;
        }
        // SAFETY: the API wrote a NUL-terminated path after the u32 cbSize,
        // inside the buffer.
        let path = unsafe {
            let start = buffer.as_ptr().add(1).cast::<u16>();
            let max = (buffer.len() - 1) * 2;
            let len = (0..max).position(|i| *start.add(i) == 0).unwrap_or(max);
            std::slice::from_raw_parts(start, len)
        };
        let mut path = path.to_vec();
        path.push(0);
        paths.push(path);
    }
    // SAFETY: the set is ours.
    unsafe { SetupDiDestroyDeviceInfoList(set) };
    paths
}

/// A stick axis byte (0 to 255, 128 centred) as -1 to 1, no dead zone, as
/// the XInput path: the title applies its own.
fn axis(v: u8) -> f32 {
    ((f32::from(v) - 128.0) / 127.0).clamp(-1.0, 1.0)
}

/// The pad state in an input report (report ID first). Reports: 0x01 (USB,
/// and the short Bluetooth one), DualSense Bluetooth 0x31, DualShock 4
/// Bluetooth 0x11. Others (feature replies and the like) are `None`.
fn decode(layout: Layout, report: &[u8]) -> Option<PadState> {
    // Where the sticks start, and which layout follows them.
    let (at, layout) = match report.first()? {
        0x01 => (1, layout),
        0x31 => (2, Layout::Full),
        0x11 => (3, Layout::Short),
        _ => return None,
    };
    let r = report.get(at..at + 10)?;
    let (sticks, triggers, buttons) = match layout {
        Layout::Short => (&r[0..4], [r[7], r[8]], &r[4..7]),
        Layout::Full => (&r[0..4], [r[4], r[5]], &r[7..10]),
    };
    // Low nibble: the D-pad, 0 up and clockwise, 8 released. High nibble:
    // square, cross, circle, triangle.
    let hat = [
        UP,
        UP | RIGHT,
        RIGHT,
        DOWN | RIGHT,
        DOWN,
        DOWN | LEFT,
        LEFT,
        UP | LEFT,
    ];
    let mut pad = hat.get(usize::from(buttons[0] & 0xF)).copied().unwrap_or(0);
    let bits = u16::from(buttons[0]) | u16::from(buttons[1]) << 8;
    for (bit, button) in [
        (0x10, X),              // square
        (0x20, A),              // cross
        (0x40, B),              // circle
        (0x80, Y),              // triangle
        (0x100, LEFT_SHOULDER), // L1
        (0x200, RIGHT_SHOULDER),
        (0x1000, VIEW), // create / share
        (0x2000, MENU), // options
        (0x4000, LEFT_STICK),
        (0x8000, RIGHT_STICK),
    ] {
        if bits & bit != 0 {
            pad |= button;
        }
    }
    Some(PadState {
        buttons: pad,
        left_trigger: f32::from(triggers[0]) / 255.0,
        right_trigger: f32::from(triggers[1]) / 255.0,
        left_x: axis(sticks[0]),
        // The pad's Y grows downwards, the title's upwards.
        left_y: -axis(sticks[1]),
        right_x: axis(sticks[2]),
        right_y: -axis(sticks[3]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Left stick full right and up, right stick centred, L2 full, R2 half;
    /// cross + circle, D-pad up-right, L1, options, R3.
    fn expected() -> PadState {
        PadState {
            buttons: A | B | UP | RIGHT | LEFT_SHOULDER | MENU | RIGHT_STICK,
            left_trigger: 1.0,
            right_trigger: 128.0 / 255.0,
            left_x: 1.0,
            left_y: 1.0,
            ..PadState::default()
        }
    }

    #[test]
    fn dualsense_usb_report() {
        let mut r = [0u8; 64];
        r[..11].copy_from_slice(&[0x01, 255, 0, 128, 128, 255, 128, 7, 0x61, 0xA1, 0]);
        assert_eq!(decode(Layout::Full, &r), Some(expected()));
    }

    #[test]
    fn dualsense_bluetooth_reports() {
        // Full report: one byte later than USB.
        let mut r = [0u8; 78];
        r[..12].copy_from_slice(&[0x31, 0x10, 255, 0, 128, 128, 255, 128, 7, 0x61, 0xA1, 0]);
        assert_eq!(decode(Layout::Short, &r), Some(expected()));
        // Short report (the pad's default over Bluetooth), padded to 78.
        let mut r = [0u8; 78];
        r[..10].copy_from_slice(&[0x01, 255, 0, 128, 128, 0x61, 0xA1, 0, 255, 128]);
        assert_eq!(decode(Layout::Short, &r), Some(expected()));
    }

    #[test]
    fn dualshock4_reports() {
        let mut r = [0u8; 64];
        r[..10].copy_from_slice(&[0x01, 255, 0, 128, 128, 0x61, 0xA1, 0, 255, 128]);
        assert_eq!(decode(Layout::Short, &r), Some(expected()));
        let mut r = [0u8; 78];
        r[..12].copy_from_slice(&[0x11, 0xC0, 0, 255, 0, 128, 128, 0x61, 0xA1, 0, 255, 128]);
        assert_eq!(decode(Layout::Short, &r), Some(expected()));
    }

    #[test]
    fn idle_square_triangle_and_other_reports() {
        let mut r = [0u8; 64];
        r[..11].copy_from_slice(&[0x01, 128, 128, 128, 128, 0, 0, 0, 0x08, 0, 0]);
        assert_eq!(decode(Layout::Full, &r), Some(PadState::default()));
        r[8] = 0x98; // square + triangle, D-pad released
        r[9] = 0x50; // create, L3
        assert_eq!(
            decode(Layout::Full, &r).unwrap().buttons,
            X | Y | VIEW | LEFT_STICK
        );
        assert_eq!(decode(Layout::Full, &[0x05, 0, 0]), None);
        assert_eq!(decode(Layout::Full, &[0x01, 128]), None);
    }

    /// Not part of the suite: what this machine has connected.
    /// `cargo test -p victory-hle --lib playstation -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn finds_a_connected_pad() {
        let paths = hid_paths();
        println!("HID interfaces: {}", paths.len());
        let pad = open().expect("no PlayStation pad");
        println!("layout {:?}, report length {}", pad.layout, pad.input_len);
        for _ in 0..3 {
            let mut buf = vec![0u8; pad.input_len.max(DUALSENSE_USB_LEN)];
            let mut len = 0u32;
            // SAFETY: as in `read`.
            unsafe {
                ReadFile(
                    pad.handle,
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    &mut len,
                    std::ptr::null_mut(),
                )
            };
            let error = std::io::Error::last_os_error();
            println!(
                "{len} bytes {:02x?} -> {:?} ({error})",
                &buf[..12],
                decode(pad.layout, &buf[..len as usize])
            );
        }
    }
}
