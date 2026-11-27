//! Pipeline-library env latches (`XCL_LIBRARY_*`). CGBJ parity; GPL-2 reimplementation.

use std::sync::OnceLock;
use std::time::Duration;

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

/// Map the library file for reads/saves (default on; `XCL_LIBRARY_MAP=0` uses buffers).
pub fn library_map() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_LIBRARY_MAP", "VICTORY_LIBRARY_MAP").is_none_or(|v| v != "0");
        if env_os("XCL_LIBRARY_MAP", "VICTORY_LIBRARY_MAP").is_some_and(|v| v == "0") {
            log_once("XCL_LIBRARY_MAP", "=0 — library I/O through buffers");
        }
        on
    })
}

/// Keep the mapped library file handle open while mapped (default on; `=0` closes after map).
pub fn library_handle() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_LIBRARY_HANDLE", "VICTORY_LIBRARY_HANDLE").is_none_or(|v| v != "0");
        if env_os("XCL_LIBRARY_HANDLE", "VICTORY_LIBRARY_HANDLE").is_some_and(|v| v == "0") {
            log_once("XCL_LIBRARY_HANDLE", "=0 — close library file after mapping");
        }
        on
    })
}

/// Serialize every library call (`XCL_LIBRARY_LOCK=1`; default off).
pub fn library_lock() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_LIBRARY_LOCK", "VICTORY_LIBRARY_LOCK").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_LIBRARY_LOCK", "1 — one library call at a time");
        }
        on
    })
}

/// Trim mapped library pages after use (default on; `XCL_LIBRARY_TRIM=0` keeps them).
pub fn library_trim() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_LIBRARY_TRIM", "VICTORY_LIBRARY_TRIM").is_none_or(|v| v != "0");
        if env_os("XCL_LIBRARY_TRIM", "VICTORY_LIBRARY_TRIM").is_some_and(|v| v == "0") {
            log_once("XCL_LIBRARY_TRIM", "=0 — leave library pages resident");
        }
        on
    })
}

/// Hand pipelines out during a save and store them afterward (`XCL_LIBRARY_STORE_LATER=1`).
pub fn library_store_later() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_LIBRARY_STORE_LATER", "VICTORY_LIBRARY_STORE_LATER").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_LIBRARY_STORE_LATER", "1 — defer library store across saves");
        }
        on
    })
}

/// Minimum seconds between library saves (`XCL_LIBRARY_SAVE_S`, default 20, floor 5).
pub fn library_save_s() -> u64 {
    static S: OnceLock<u64> = OnceLock::new();
    *S.get_or_init(|| {
        let seconds = env_str("XCL_LIBRARY_SAVE_S", "VICTORY_LIBRARY_SAVE_S")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(20)
            .max(5);
        if env_str("XCL_LIBRARY_SAVE_S", "VICTORY_LIBRARY_SAVE_S").is_some() {
            log_once("XCL_LIBRARY_SAVE_S", &format!("{seconds}s between library saves"));
        }
        seconds
    })
}

pub fn library_save_every() -> Duration {
    Duration::from_secs(library_save_s())
}

/// Touch library latches once (PSO / boot path).
pub fn note_boot() {
    let _ = (
        library_map(),
        library_handle(),
        library_lock(),
        library_trim(),
        library_store_later(),
        library_save_s(),
    );
}
