//! Readback / queue / ready / priority latches (`XCL_READ_*`, `XCL_QUEUE_WAIT`, …). CGBJ parity (GPL-2).

use std::sync::OnceLock;

fn log_once(name: &'static str, detail: &str) {
    static SEEN: OnceLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let set = SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    if let Ok(mut g) = set.lock() {
        if g.insert(name) {
            crate::xlog_info!("D3D12.X: {name} — {detail}");
        }
    }
}

fn env_os(xcl: &str, victory: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(xcl).or_else(|| std::env::var_os(victory))
}

fn env_str(xcl: &str, victory: &str) -> Option<String> {
    std::env::var(xcl).ok().or_else(|| std::env::var(victory).ok())
}

/// Texture→footprint GPU copies into readback (default on; `XCL_READ_BACK=0` refuses).
pub fn read_back() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_READ_BACK", "VICTORY_READ_BACK").is_none_or(|v| v != "0");
        if env_os("XCL_READ_BACK", "VICTORY_READ_BACK").is_some_and(|v| v == "0") {
            log_once("XCL_READ_BACK", "=0 — texture readback copies off");
        }
        on
    })
}

/// Cap per-thread CPU read scratch after use (`XCL_READ_KEEP_MB`); unset keeps peak size.
pub fn read_keep_bytes() -> Option<usize> {
    static MOST: OnceLock<Option<usize>> = OnceLock::new();
    *MOST.get_or_init(|| {
        let n = env_str("XCL_READ_KEEP_MB", "VICTORY_READ_KEEP_MB")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .map(|mb| mb.saturating_mul(1 << 20));
        if let Some(bytes) = n {
            log_once("XCL_READ_KEEP_MB", &format!("{} MiB per-thread read scratch cap", bytes >> 20));
        }
        n
    })
}

/// Log CPU reads of GPU-written textures (`XCL_READ_WATCH=1`; default off).
pub fn read_watch() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_READ_WATCH", "VICTORY_READ_WATCH").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_READ_WATCH", "1 — watch CPU reads of GPU texture memory");
        }
        on
    })
}

/// Pool idle texture readback buffers (`XCL_READBACK_POOL=1`; default off).
pub fn readback_pool() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_READBACK_POOL", "VICTORY_READBACK_POOL").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_READBACK_POOL", "1 — reuse idle texture readback buffers");
        }
        on
    })
}

/// Idle readback pool budget in bytes (`XCL_READBACK_POOL_MB`, default 256 when pool on).
pub fn readback_pool_bytes() -> u64 {
    static LIMIT: OnceLock<u64> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if !readback_pool() {
            return 0;
        }
        let mb = env_str("XCL_READBACK_POOL_MB", "VICTORY_READBACK_POOL_MB")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(256);
        if env_str("XCL_READBACK_POOL_MB", "VICTORY_READBACK_POOL_MB").is_some() {
            log_once("XCL_READBACK_POOL_MB", &format!("{mb} MiB readback pool"));
        }
        mb << 20
    })
}

/// Skip re-walking unchanged table entries at draw (default on; `XCL_READY_ONCE=0` every draw).
pub fn ready_once() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_READY_ONCE", "VICTORY_READY_ONCE").is_none_or(|v| v != "0");
        if env_os("XCL_READY_ONCE", "VICTORY_READY_ONCE").is_some_and(|v| v == "0") {
            log_once("XCL_READY_ONCE", "=0 — recheck every table entry each draw");
        }
        on
    })
}

/// Recheck a quietly-changed texture next frame (default on; `=0` after four frames).
pub fn recheck_soon() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_RECHECK_SOON", "VICTORY_RECHECK_SOON").is_none_or(|v| v != "0");
        if env_os("XCL_RECHECK_SOON", "VICTORY_RECHECK_SOON").is_some_and(|v| v == "0") {
            log_once("XCL_RECHECK_SOON", "=0 — recheck quietly-changed textures after four frames");
        }
        on
    })
}

/// Queues wait on fences before continuing (default on; `XCL_QUEUE_WAIT=0` fire-and-forget).
pub fn queue_wait() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_QUEUE_WAIT", "VICTORY_QUEUE_WAIT").is_none_or(|v| v != "0");
        if env_os("XCL_QUEUE_WAIT", "VICTORY_QUEUE_WAIT").is_some_and(|v| v == "0") {
            log_once("XCL_QUEUE_WAIT", "=0 — queues do not wait on fences");
        }
        on
    })
}

/// Raise process to above-normal unless `XCL_PROCESS_PRIORITY=normal` / `0`.
pub fn process_priority_above_normal() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let leave = env_os("XCL_PROCESS_PRIORITY", "VICTORY_PROCESS_PRIORITY")
            .is_some_and(|v| v == "normal" || v == "0");
        if leave {
            log_once("XCL_PROCESS_PRIORITY", "normal — leave process priority unchanged");
        }
        !leave
    })
}

/// Apply [`process_priority_above_normal`] once (Windows).
pub fn apply_process_priority() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        if !process_priority_above_normal() {
            return;
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS,
            };
            // SAFETY: process handle is current process; priority class is a documented constant.
            let set = unsafe { SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS) } != 0;
            crate::xlog_info!(
                "D3D12.X: XCL_PROCESS_PRIORITY — above-normal (ok={set}; =normal to leave)"
            );
        }
    });
}

pub fn note_boot() {
    apply_process_priority();
    let _ = (
        read_back(),
        read_keep_bytes(),
        read_watch(),
        readback_pool(),
        readback_pool_bytes(),
        ready_once(),
        recheck_soon(),
        queue_wait(),
    );
}
