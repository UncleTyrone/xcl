//! View / descriptor env latches (CGBJ `VICTORY_VIEW_*` parity; GPL-2).

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

fn default_on(xcl: &'static str, victory: &'static str, detail_on: &str) -> bool {
    let on = env_os(xcl, victory).is_none_or(|v| v != "0");
    if env_os(xcl, victory).is_some_and(|v| v == "0") {
        log_once(xcl, "=0 — off");
    } else if env_os(xcl, victory).is_some() {
        log_once(xcl, detail_on);
    }
    on
}

/// Views off their copy's elements get their own bytes (`XCL_VIEW_BYTES`; default on).
pub fn view_bytes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_BYTES", "VICTORY_VIEW_BYTES", "own-byte copies for off-element views"))
}

/// Stage descriptor copies instead of in-place (`XCL_VIEW_COPY`; default on; `=0` in place).
pub fn view_copy() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_VIEW_COPY", "VICTORY_VIEW_COPY").is_none_or(|v| v != "0");
        if env_os("XCL_VIEW_COPY", "VICTORY_VIEW_COPY").is_some_and(|v| v == "0") {
            log_once("XCL_VIEW_COPY", "=0 — make views in place");
        } else if env_os("XCL_VIEW_COPY", "VICTORY_VIEW_COPY").is_some() {
            log_once("XCL_VIEW_COPY", "stage descriptor copies");
        }
        on
    })
}

/// Bring buffer views up to date by host-copy extent (`XCL_VIEW_EXTENT`; default on).
pub fn view_extent() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_EXTENT", "VICTORY_VIEW_EXTENT", "refresh views by host-copy extent"))
}

/// Guard all views of a page together (`XCL_VIEW_GUARD`; default on).
pub fn view_guard() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_GUARD", "VICTORY_VIEW_GUARD", "guard all views of a page together"))
}

/// Inline constant / placed views in records (`XCL_VIEW_INLINE`; default on).
pub fn view_inline() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_INLINE", "VICTORY_VIEW_INLINE", "inline constant/placed views in records"))
}

/// Null-slot clamp for translated shaders (`XCL_VIEW_LIMIT=<slot>`; unset = no clamp).
pub fn view_limit() -> Option<u32> {
    static LIMIT: OnceLock<Option<u32>> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        let n = env_var("XCL_VIEW_LIMIT", "VICTORY_VIEW_LIMIT")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_VIEW_LIMIT", &format!("null-slot clamp {n}"));
        }
        n
    })
}

/// Descriptor-build worker threads (`XCL_VIEW_THREADS`; default 4, or 6 when `records`).
pub fn view_threads(records: bool) -> usize {
    static N: OnceLock<(usize, usize)> = OnceLock::new();
    let (views, recs) = *N.get_or_init(|| {
        let asked = env_var("XCL_VIEW_THREADS", "VICTORY_VIEW_THREADS")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        let views = asked.unwrap_or(4usize).clamp(1, 16);
        let recs = asked.unwrap_or(6usize).clamp(1, 16);
        if asked.is_some() {
            log_once("XCL_VIEW_THREADS", &format!("{views} view / {recs} record workers"));
        }
        (views, recs)
    });
    if records { recs } else { views }
}

/// Touch latches from descriptor / watch paths.
pub fn on_view_path() {
    let _ = (
        view_bytes(),
        view_copy(),
        view_extent(),
        view_guard(),
        view_inline(),
        view_limit(),
        view_threads(false),
    );
}
