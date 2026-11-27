//! Keep-* residency / descriptor latches (`XCL_KEEP_*`). CGBJ parity (GPL-2).

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

/// Keep freed card memory while there is room (default on; `XCL_KEEP_FREED=0` returns it).
pub fn keep_freed() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_KEEP_FREED", "VICTORY_KEEP_FREED").is_none_or(|v| v != "0");
        if env_os("XCL_KEEP_FREED", "VICTORY_KEEP_FREED").is_some_and(|v| v == "0") {
            log_once("XCL_KEEP_FREED", "=0 — return freed card memory promptly");
        }
        on
    })
}

/// How aggressively a rewritten slot keeps showing the old texture (default 1).
/// 0 never; 1 same-kind elsewhere/size; 2 any same dimension.
pub fn keep_shown() -> u8 {
    static KEEP: OnceLock<u8> = OnceLock::new();
    *KEEP.get_or_init(|| {
        let v = env_str("XCL_KEEP_SHOWN", "VICTORY_KEEP_SHOWN")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1);
        if env_str("XCL_KEEP_SHOWN", "VICTORY_KEEP_SHOWN").is_some() {
            log_once("XCL_KEEP_SHOWN", &format!("{v} — rewritten-slot keep policy"));
        }
        v
    })
}

/// Keep GPU/shadow snapshots between uses (`XCL_KEEP_SNAPSHOTS=1`; default off).
pub fn keep_snapshots() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_KEEP_SNAPSHOTS", "VICTORY_KEEP_SNAPSHOTS").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_KEEP_SNAPSHOTS", "1 — retain shadow/GPU snapshots");
        }
        on
    })
}

/// Cap for kept snapshots in bytes (`XCL_KEEP_SNAPSHOTS_MB`, default 256 MiB when snapshots on).
pub fn keep_snapshots_bytes() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| {
        let mb = env_str("XCL_KEEP_SNAPSHOTS_MB", "VICTORY_KEEP_SNAPSHOTS_MB")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(256);
        if env_str("XCL_KEEP_SNAPSHOTS_MB", "VICTORY_KEEP_SNAPSHOTS_MB").is_some() {
            log_once("XCL_KEEP_SNAPSHOTS_MB", &format!("{mb} MiB snapshot cap"));
        }
        mb << 20
    })
}

/// Keep prior view on a rewritten same-placed slot until the new view is ready (default on).
pub fn keep_views() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_KEEP_VIEWS", "VICTORY_KEEP_VIEWS").is_none_or(|v| v != "0");
        if env_os("XCL_KEEP_VIEWS", "VICTORY_KEEP_VIEWS").is_some_and(|v| v == "0") {
            log_once("XCL_KEEP_VIEWS", "=0 — null rewritten slots immediately");
        }
        on
    })
}

/// Touch keep latches (log-once) from a refresh/boot path.
pub fn note_boot() {
    let _ = (
        keep_freed(),
        keep_shown(),
        keep_snapshots(),
        keep_snapshots_bytes(),
        keep_views(),
    );
}
