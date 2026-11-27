//! Presented-frame capture (`XCL_CAPTURE=<dir>`).
//!
//! Powers-of-two frames, `XCL_CAPTURE_FRAMES` list, and world-entry frames
//! from `XCL_CAPTURE_WORLD` / `XCL_CAPTURE_WORLD_ALSO`. Pictures land via
//! the present readback path (`present::write_frame` → [`save_bmp`]).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::{diag_env, misc_env};

/// Capture output folder, if set.
pub(crate) fn folder() -> Option<&'static PathBuf> {
    misc_env::capture()
}

/// Explicit frame list from `XCL_CAPTURE_FRAMES`.
fn listed_frames() -> &'static [u64] {
    diag_env::capture_frames()
}

fn world_on() -> bool {
    diag_env::capture_world()
}

fn world_also() -> &'static [u64] {
    diag_env::capture_world_also()
}

/// First heavy world frame (500+ draws), or `u64::MAX` until seen.
static WORLD: AtomicU64 = AtomicU64::new(u64::MAX);

/// At present of `frame`: draws run on the host so far (cumulative).
pub(crate) fn frame_drew(frame: u64, drawn: u64) {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let before = LAST.swap(drawn, Ordering::Relaxed);
    if world_on()
        && drawn.saturating_sub(before) >= 500
        && WORLD.compare_exchange(u64::MAX, frame, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    {
        crate::xlog_info!(
            "D3D12.X: XCL_CAPTURE_WORLD — first ≥500-draw frame {frame}; capturing +2/+6 (+30/+80)"
        );
    }
}

/// Whether frame `n` (1-based present boundary, or shown index) should be saved.
pub(crate) fn wanted(n: u64) -> bool {
    if folder().is_none() {
        return false;
    }
    if n.is_power_of_two() {
        return true;
    }
    if listed_frames().contains(&n) {
        return true;
    }
    let first = WORLD.load(Ordering::Relaxed);
    if world_on() {
        if first == u64::MAX {
            // Menus / loading: every 16th frame.
            return n.is_multiple_of(16);
        }
        let after = n.wrapping_sub(first);
        if [2, 6, 30, 80].contains(&after) || world_also().contains(&after) {
            return true;
        }
        // Opening cutscene cadence.
        if n > first && after.is_multiple_of(8) {
            return true;
        }
    }
    false
}

/// Sidecar JSON beside `frame-<n>.bmp` (timeline / record / panel summary).
pub(crate) fn save_companion(n: u64, body: &str) {
    let Some(dir) = folder() else { return };
    if !wanted(n) {
        return;
    }
    let path = dir.join(format!("frame-{n}.json"));
    if std::fs::write(&path, body).is_ok() {
        crate::xlog_info!("D3D12.X: XCL_CAPTURE companion {}", path.display());
    }
}

/// Write a BMP already built by present into the capture folder as `frame-<n>.bmp`.
pub(crate) fn save_bmp(n: u64, bmp: &[u8]) {
    let Some(dir) = folder() else { return };
    if let Err(e) = std::fs::create_dir_all(dir) {
        crate::xlog_warn!("D3D12.X: XCL_CAPTURE mkdir {}: {e}", dir.display());
        return;
    }
    let path = dir.join(format!("frame-{n}.bmp"));
    match std::fs::write(&path, bmp) {
        Ok(()) => crate::xlog_info!("D3D12.X: XCL_CAPTURE wrote {}", path.display()),
        Err(e) => crate::xlog_warn!("D3D12.X: XCL_CAPTURE write {}: {e}", path.display()),
    }
}

/// Ops log line for a listed/world capture frame (best-effort text companion).
pub(crate) fn op_line(frame: u64, list: usize, what: &str) {
    let Some(dir) = folder() else { return };
    if !listed_frames().contains(&frame) && WORLD.load(Ordering::Relaxed) == u64::MAX {
        return;
    }
    if !wanted(frame) && !listed_frames().contains(&frame) {
        return;
    }
    let path = dir.join(format!("frame-{frame}-ops.txt"));
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "list {list:#x} {what}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_of_two_wanted_only_with_folder() {
        // Without XCL_CAPTURE, never.
        assert!(!wanted(1));
        assert!(!wanted(16));
    }

    #[test]
    fn listed_helper_is_stable() {
        let _ = listed_frames();
        let _ = world_also();
        let _ = folder().map(|p| p.display().to_string());
    }
}
