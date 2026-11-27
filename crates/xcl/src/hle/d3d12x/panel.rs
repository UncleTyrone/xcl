//! Host diagnostics panel (`XCL_PANEL=1` / `VICTORY_PANEL`).
//!
//! CGBJ draws white bitmap text onto the swap chain (F3 toggles). XCL currently samples the same
//! counter set into the log twice a second when the latch is on, and toggles visibility with F3
//! when an XCL/title window is foreground — GPU blit overlay lands with the full host panel port.
//!
//! Study-only reference: CGBJ `victory-gpu` `host/panel.rs` (GPL-3); this file is GPL-2.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const VK_F3: u16 = 0x72;
const EVERY: Duration = Duration::from_millis(500);

#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}

fn asked() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| super::misc_env::panel() || super::remaining_env::panel())
}

fn shown() -> &'static AtomicBool {
    static SHOWN: OnceLock<AtomicBool> = OnceLock::new();
    SHOWN.get_or_init(|| AtomicBool::new(asked()))
}

fn f3_edge() -> bool {
    static DOWN: AtomicBool = AtomicBool::new(false);
    let down = unsafe { GetAsyncKeyState(i32::from(VK_F3)) } < 0;
    if DOWN.swap(down, Ordering::Relaxed) || !down {
        return false;
    }
    let mut process = 0u32;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
    process == std::process::id()
}

/// Called once per PresentX / present boundary.
pub(crate) fn on_frame(frame: u64) {
    if !asked() {
        return;
    }
    if f3_edge() {
        let now = !shown().load(Ordering::Relaxed);
        shown().store(now, Ordering::Relaxed);
        crate::xlog_info!("D3D12.X: panel {}", if now { "shown (F3)" } else { "hidden (F3)" });
    }
    if !shown().load(Ordering::Relaxed) {
        return;
    }
    static LAST: OnceLock<std::sync::Mutex<Instant>> = OnceLock::new();
    let last = LAST.get_or_init(|| std::sync::Mutex::new(Instant::now()));
    let mut g = last.lock().unwrap_or_else(|e| e.into_inner());
    if g.elapsed() < EVERY {
        return;
    }
    *g = Instant::now();
    let (presents, _) = super::present::counters();
    let (draws, dispatches) = super::bind::activity();
    static PREV: OnceLock<std::sync::Mutex<(u64, u64, u64, Instant)>> = OnceLock::new();
    let prev = PREV.get_or_init(|| std::sync::Mutex::new((presents, draws, dispatches, Instant::now())));
    let mut p = prev.lock().unwrap_or_else(|e| e.into_inner());
    let dt = p.3.elapsed().as_secs_f64().max(0.001);
    let fps = presents.saturating_sub(p.0) as f64 / dt;
    let frames = (presents.saturating_sub(p.0)).max(1) as f64;
    let d_draws = draws.saturating_sub(p.1) as f64 / frames;
    let d_disp = dispatches.saturating_sub(p.2) as f64 / frames;
    *p = (presents, draws, dispatches, Instant::now());
    let mem = match (super::real::video_memory(), super::real::video_budget()) {
        (Some((local, non_local)), Some((loc, _))) => format!(
            "vram local={}MB non-local={}MB budget={}MB",
            local >> 20,
            non_local >> 20,
            loc.Budget >> 20
        ),
        (Some((local, non_local)), None) => {
            format!("vram local={}MB non-local={}MB", local >> 20, non_local >> 20)
        }
        _ => "vram unknown".into(),
    };
    let line = format!(
        "FPS {fps:.1}  DRAWS {d_draws:.0}  DISPATCH {d_disp:.0}  frame={frame}  {mem}"
    );
    crate::xlog_info!("XCL PANEL {line}");
    if let Some(hwnd) = super::present::window_hwnd() {
        let title = format!("XCL | {line}");
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(hwnd, wide.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn asked_defaults_off_without_env() {
        // Latch is OnceLock'd from env; without XCL_PANEL the panel stays quiet.
        assert!(!super::asked() || std::env::var_os("XCL_PANEL").is_some() || std::env::var_os("VICTORY_PANEL").is_some());
    }
}
