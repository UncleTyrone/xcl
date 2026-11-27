//! Pop-watch diagnostics (`XCL_POP_*`). CGBJ parity (GPL-2); latches only until present path grows.

use std::path::PathBuf;
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

fn parse_windows(text: &str) -> Vec<(f64, f64)> {
    text.split(',')
        .filter_map(|part| {
            let (from, to) = part.trim().split_once('-')?;
            Some((from.trim().parse().ok()?, to.trim().parse().ok()?))
        })
        .collect()
}

/// `XCL_POP_WATCH=<from>-<to>[,…]` seconds since start; empty → off.
pub fn pop_watch() -> &'static [(f64, f64)] {
    static ASKED: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    ASKED
        .get_or_init(|| {
            let windows = env_str("XCL_POP_WATCH", "VICTORY_POP_WATCH")
                .map(|v| parse_windows(&v))
                .unwrap_or_default();
            if !windows.is_empty() {
                log_once(
                    "XCL_POP_WATCH",
                    &format!("{} time window(s) for present pop compare", windows.len()),
                );
            }
            windows
        })
        .as_slice()
}

/// Folder for blip triplets (`XCL_POP_FOLDER`); none unless set.
pub fn pop_folder() -> Option<&'static PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let p = env_os("XCL_POP_FOLDER", "VICTORY_POP_FOLDER").map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_POP_FOLDER", &format!("save blip frames under {}", path.display()));
        }
        p
    })
    .as_ref()
}

/// Save every watched frame's triplet (`XCL_POP_ALL=1`; default off → blips only).
pub fn pop_all() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_POP_ALL", "VICTORY_POP_ALL").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_POP_ALL", "1 — save every watched present triplet");
        }
        on
    })
}

/// Whether pop-watch is configured (any time window).
pub fn pop_watch_configured() -> bool {
    !pop_watch().is_empty()
}

pub fn note_boot() {
    let _ = (pop_watch_configured(), pop_folder(), pop_all());
}
