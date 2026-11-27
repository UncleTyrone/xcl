//! Fake GPU busy time for title timestamp pairs (`XCL_GPU_LOAD_MS`).

use std::sync::OnceLock;

/// Ticks subtracted from a timestamp pair's start slot (`va & 8 != 0`).
pub fn load_ticks() -> u64 {
    static TICKS: OnceLock<u64> = OnceLock::new();
    *TICKS.get_or_init(|| {
        let ms = std::env::var("XCL_GPU_LOAD_MS")
            .or_else(|_| std::env::var("VICTORY_GPU_LOAD_MS"))
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if ms == 0 {
            return 0;
        }
        let mut freq = 0i64;
      // SAFETY: QueryPerformanceFrequency out pointer is valid.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut freq) };
        let ticks = ms * freq.max(1) as u64 / 1000;
        crate::xlog_info!("D3D12.X: XCL_GPU_LOAD_MS={ms} ({ticks} QPC ticks)");
        ticks
    })
}

pub fn adjust_timestamp(va: usize, value: u64) -> u64 {
    let load = load_ticks();
    if load == 0 {
        return value;
    }
    if va & 8 != 0 {
        value.saturating_sub(load).max(1)
    } else {
        value
    }
}