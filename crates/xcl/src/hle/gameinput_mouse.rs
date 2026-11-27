//! GameInput mouse device readings (see `gameinput.rs` for COM wiring).

use std::sync::atomic::{AtomicU32, Ordering};

/// `GameInputKindMouse`
pub const KIND_MOUSE: u32 = 0x20;

/// `GameInputMouseState` (40 bytes)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct MouseState {
    pub buttons: u32,
    pub position_x: i64,
    pub position_y: i64,
    pub wheel_x: i64,
    pub wheel_y: i64,
}

pub const MOUSE_LEFT: u32 = 0x1;
pub const MOUSE_RIGHT: u32 = 0x2;
pub const MOUSE_MIDDLE: u32 = 0x4;
pub const MOUSE_BUTTON4: u32 = 0x8;
pub const MOUSE_BUTTON5: u32 = 0x10;

const MOUSE_DEVICE_ID: [u8; 32] = *b"XCL mouse 0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

pub fn mouse_device_id() -> [u8; 32] {
    MOUSE_DEVICE_ID
}

pub const MOUSE_DEVICE_ID_BYTES: [u8; 32] = MOUSE_DEVICE_ID;

/// Sample the host cursor for GameInput (always available; not gated on `keyboard_mouse`).
pub fn poll_mouse() -> MouseState {
    let mut s = MouseState::default();
    if crate::hle::kbm::gameinput_mouse_sample(&mut s) {
        static SEEN: AtomicU32 = AtomicU32::new(0);
        let new = s.buttons & !SEEN.fetch_or(s.buttons, Ordering::Relaxed);
        if new != 0 {
            crate::xlog_info!("GameInput: mouse buttons {new:#x} pressed for the first time");
        }
    }
    s
}