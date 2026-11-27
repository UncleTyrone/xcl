//! GameInput keyboard device readings (see `gameinput.rs` for COM wiring).

/// `GameInputKindKeyboard` (GDK v0)
pub const KIND_KEYBOARD: u32 = 0x10;

/// `GameInputKeyState` (12 bytes)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct KeyState {
    pub scan_code: u32,
    pub code_point: u32,
    pub virtual_key: u8,
    pub dead_key: bool,
}

const KEYBOARD_DEVICE_ID: [u8; 32] = [
    b'X', b'C', b'L', b' ', b'k', b'e', b'y', b'b', b'o', b'a', b'r', b'd', b' ', b'0', 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0,
];

pub fn keyboard_device_id() -> [u8; 32] {
    KEYBOARD_DEVICE_ID
}

pub const KEYBOARD_DEVICE_ID_BYTES: [u8; 32] = KEYBOARD_DEVICE_ID;

use std::ffi::c_void;

#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(window: *mut c_void, pid: *mut u32) -> u32;
    fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
}

/// Host keys while a window of this process has the focus (not gated on `keyboard_mouse`).
pub fn poll_keys() -> Vec<KeyState> {
    if !focused() {
        return Vec::new();
    }
    const MAPVK_VK_TO_VSC_EX: u32 = 4;
    let mut keys = Vec::new();
    for vk in 0x08u8..=0xFE {
        if (0x10..=0x12).contains(&vk) {
            continue;
        }
        // SAFETY: plain async key query.
        if unsafe { GetAsyncKeyState(i32::from(vk)) } >= 0 {
            continue;
        }
        keys.push(KeyState {
            scan_code: unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC_EX) },
            virtual_key: vk,
            ..Default::default()
        });
    }
    keys.sort_unstable_by_key(|k| k.virtual_key);
    keys
}

fn focused() -> bool {
    // SAFETY: Win32 foreground queries.
    unsafe {
        let w = GetForegroundWindow();
        if w.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(w, &mut pid);
        pid == GetCurrentProcessId()
    }
}