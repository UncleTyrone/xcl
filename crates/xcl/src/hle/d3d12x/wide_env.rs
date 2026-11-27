//! Wide parallel refresh / merge env latches (CGBJ `VICTORY_WIDE_*` parity; GPL-2).
//!
//! `XCL_WIDE_BACK` already lives in `wide_back` / `port` (MATCH).

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

fn env_var(xcl: &str, victory: &str) -> Result<String, std::env::VarError> {
    std::env::var(xcl).or_else(|_| std::env::var(victory))
}

/// Parallel host-copy refreshes (`XCL_WIDE_REFRESH`: unset → on if ≥4 CPUs; `1` force; `0` off).
pub fn wide_refresh() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let setting = env_var("XCL_WIDE_REFRESH", "VICTORY_WIDE_REFRESH").ok();
        let cpus = std::thread::available_parallelism().ok().map(|n| n.get());
        let on = match setting.as_deref().map(str::trim) {
            Some("1") => true,
            Some("0") => false,
            _ => cpus.is_some_and(|n| n >= 4),
        };
        if setting.is_some() {
            log_once(
                "XCL_WIDE_REFRESH",
                if on {
                    "parallel host-copy refreshes"
                } else {
                    "serial host-copy refreshes"
                },
            );
        }
        on
    })
}

/// Parallel upload fills when [`wide_refresh`] (`XCL_WIDE_FILLS`; default on).
pub fn wide_fills() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_WIDE_FILLS", "VICTORY_WIDE_FILLS").is_none_or(|v| v != "0") && wide_refresh();
        if env_os("XCL_WIDE_FILLS", "VICTORY_WIDE_FILLS").is_some_and(|v| v == "0") {
            log_once("XCL_WIDE_FILLS", "=0 — serial upload fills");
        }
        on
    })
}

/// Parallel merge of shader writes into title memory (`XCL_WIDE_MERGE=1`; default off).
pub fn wide_merge() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_WIDE_MERGE", "VICTORY_WIDE_MERGE").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_WIDE_MERGE", "=1 — parallel merge into title memory");
        }
        on
    })
}

/// Spin-wait microseconds before parking wide workers (`XCL_WIDE_SPIN`; default 0).
pub fn wide_spin_us() -> u64 {
    static US: OnceLock<u64> = OnceLock::new();
    *US.get_or_init(|| {
        let us = env_var("XCL_WIDE_SPIN", "VICTORY_WIDE_SPIN")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        if us > 0 {
            log_once("XCL_WIDE_SPIN", &format!("{us} µs spin before park"));
        }
        us
    })
}

/// Parallel texture refresh when [`wide_refresh`] (`XCL_WIDE_TEXTURES`; default on).
pub fn wide_textures() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_WIDE_TEXTURES", "VICTORY_WIDE_TEXTURES").is_none_or(|v| v != "0") && wide_refresh();
        if env_os("XCL_WIDE_TEXTURES", "VICTORY_WIDE_TEXTURES").is_some_and(|v| v == "0") {
            log_once("XCL_WIDE_TEXTURES", "=0 — serial texture refreshes");
        }
        on
    })
}

/// Touch latches from refresh / land paths.
pub fn on_refresh_path() {
    let _ = (wide_refresh(), wide_fills(), wide_merge(), wide_spin_us(), wide_textures());
}
