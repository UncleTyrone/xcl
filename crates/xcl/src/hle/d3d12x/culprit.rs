//! GPU stall dispatch markers (`XCL_GPU_CULPRIT=1`). Full marker readback is on amd-dev; flight
//! calls `on_watchdog_release` when the GPU has not progressed for 3 s.

pub fn enabled() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var("XCL_GPU_CULPRIT").is_ok_and(|v| v.trim() == "1"))
}

/// Watchdog hook from `flight` when `XCL_GPU_CULPRIT=1` (markers not ported here yet).
pub fn on_watchdog_release() {
    let _ = enabled();
}