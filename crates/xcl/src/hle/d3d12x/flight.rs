//! Queue fence hook for `XCL_FLIGHT` / `XCL_ACTIVITY` (full flight recorder on amd-dev).

use windows::Win32::Graphics::Direct3D12::ID3D12Fence;

/// `queue` was given `Signal(fence, value)` after its work (`name`: whose queue it is).
pub fn signaled(_name: &'static str, _queue: usize, _fence: &ID3D12Fence, _value: u64) {}