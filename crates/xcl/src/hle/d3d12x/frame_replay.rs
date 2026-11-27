//! Frame record / replay scaffold (`XCL_RECORD` / `XCL_REPLAY`). **Default off.**
//!
//! Minimal hooks for deterministic submit/present regression work. Writes `index.jsonl` under
//! the record folder with submit and present events (queues, list addresses, counts).
//!
//! **Gaps vs CGBJ `victory-gpu/replay/*`:** no host-list capture, no per-pass GPU timestamps,
//! no re-execute merge against live D3D12, no shader variant census, no `victory replay` CLI.

use std::ffi::c_void;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
struct Settings {
    record_dir: Option<PathBuf>,
    replay_dir: Option<PathBuf>,
    skip_presents: u64,
    max_frames: Option<u64>,
    always: bool,
}

fn parse_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn settings() -> &'static Settings {
    static S: OnceLock<Settings> = OnceLock::new();
    S.get_or_init(|| {
        let record_dir = std::env::var("XCL_RECORD")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let replay_dir = std::env::var("XCL_REPLAY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let always = std::env::var("XCL_RECORD_ALWAYS").is_ok_and(|v| v.trim() == "1");
        Settings {
            record_dir,
            replay_dir,
            skip_presents: parse_u64("XCL_RECORD_AT").unwrap_or(0),
            max_frames: parse_u64("XCL_RECORD_FRAMES"),
            always,
        }
    })
}

static LOGGED: OnceLock<()> = OnceLock::new();
static PRESENTS: AtomicU64 = AtomicU64::new(0);
static SUBMITS: AtomicU64 = AtomicU64::new(0);
static FRAMES_RECORDED: AtomicU64 = AtomicU64::new(0);
static WRITER: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn ensure_log() {
    LOGGED.get_or_init(|| {
        let s = settings();
        if let Some(dir) = &s.record_dir {
            if create_dir_all(dir).is_ok() {
                let path = dir.join("index.jsonl");
                if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
                    *WRITER.lock().unwrap() = Some(f);
                    crate::xlog_info!("D3D12.X: frame record XCL_RECORD → {}", path.display());
                } else {
                    crate::xlog_warn!("D3D12.X: XCL_RECORD could not open {}", path.display());
                }
            }
        }
        if let Some(dir) = &s.replay_dir {
            crate::xlog_info!(
                "D3D12.X: XCL_REPLAY={} (scaffold only — re-execute not implemented)",
                dir.display()
            );
        }
    });
}

fn recording_active() -> bool {
    let s = settings();
    let Some(_) = s.record_dir else { return false };
    if !s.always {
        if let Some(max) = s.max_frames {
            if FRAMES_RECORDED.load(Ordering::Relaxed) >= max {
                return false;
            }
        } else if FRAMES_RECORDED.load(Ordering::Relaxed) >= 1 {
            return false;
        }
    }
    true
}

fn append_line(line: &str) {
    ensure_log();
    if let Ok(mut g) = WRITER.lock() {
        if let Some(f) = g.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
}

pub fn record_enabled() -> bool {
    settings().record_dir.is_some()
}

pub fn replay_enabled() -> bool {
    settings().replay_dir.is_some()
}

/// Title `ExecuteCommandLists` body (after own-work filter).
pub fn on_title_submit(queue: *mut c_void, lists: &[*mut c_void]) {
    if !recording_active() {
        return;
    }
    let n = SUBMITS.fetch_add(1, Ordering::Relaxed) + 1;
    let addrs: Vec<usize> = lists.iter().map(|p| *p as usize).collect();
    append_line(&format!(
        "{{\"kind\":\"submit\",\"n\":{n},\"queue\":{},\"lists\":{lists_len},\"addrs\":{addrs:?}}}",
        queue as usize,
        lists_len = lists.len(),
    ));
}

/// After `PresentX` token is known.
pub fn on_present(token: u64) {
    if !record_enabled() {
        return;
    }
    let p = PRESENTS.fetch_add(1, Ordering::Relaxed) + 1;
    if p <= settings().skip_presents {
        return;
    }
    if !recording_active() {
        return;
    }
    let frame = FRAMES_RECORDED.fetch_add(1, Ordering::Relaxed) + 1;
    append_line(&format!(
        "{{\"kind\":\"present\",\"frame\":{frame},\"token\":{token},\"submits\":{}}}",
        SUBMITS.load(Ordering::Relaxed),
    ));
    if !settings().always {
        if settings().max_frames.is_none_or(|m| frame >= m) {
            crate::xlog_info!("D3D12.X: frame record stopped after {frame} frame(s) (set XCL_RECORD_ALWAYS=1 to continue)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_off_without_env() {
        if std::env::var_os("XCL_RECORD").is_none() {
            assert!(!record_enabled());
        }
    }
}