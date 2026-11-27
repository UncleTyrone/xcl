//! Queue fence hook for `XCL_FLIGHT` / `XCL_ACTIVITY` (full recorder on amd-dev / main).

use windows::Win32::Graphics::Direct3D12::ID3D12Fence;

pub fn on() -> bool {
    std::env::var("XCL_FLIGHT").map_or(std::env::var_os("XCL_ACTIVITY").is_some(), |v| v.trim() != "0")
}

/// `queue` was given `Signal(fence, value)` after its work (`name`: whose queue it is).
pub fn signaled(_name: &'static str, _queue: usize, _fence: &ID3D12Fence, _value: u64) {}

pub fn note_event(_kind: &'static str, _detail: String) {}

pub fn note_tiles(_caller: &'static str, _tiles: u64, _ms: f64) {}

pub fn note_resource(_kind: &'static str, _ms: f64) {}

pub fn dump_on_removal() {}
