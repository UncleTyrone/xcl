//! Page / map / open-view latches (`XCL_PAGE_*`, `XCL_MAP_*`, `XCL_OPEN_ONE_VIEW`). CGBJ parity (GPL-2).

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

/// Keep 4 KiB parts inside a 64 KiB page on refresh (default on; `=0` copies whole pages).
pub fn page_parts() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_PAGE_PARTS", "VICTORY_PAGE_PARTS").is_none_or(|v| v != "0");
        if env_os("XCL_PAGE_PARTS", "VICTORY_PAGE_PARTS").is_some_and(|v| v == "0") {
            log_once("XCL_PAGE_PARTS", "=0 — refresh whole 64 KiB pages");
        }
        on
    })
}

/// Bit mask for unguarded title-memory writes (`XCL_PAGE_WRITES`).
/// unset/`guards` → 1; `0` → 0; `1` → 3; `slots` → 2; `all` → 7.
pub fn page_writes() -> u8 {
    static MODE: OnceLock<u8> = OnceLock::new();
    *MODE.get_or_init(|| {
        let mode = match env_str("XCL_PAGE_WRITES", "VICTORY_PAGE_WRITES")
            .as_deref()
            .map(str::trim)
        {
            Some("0") => 0,
            Some("1") => 3,
            Some("slots") => 2,
            Some("all") => 7,
            Some("guards") | None => 1,
            Some(_) => 1,
        };
        if env_str("XCL_PAGE_WRITES", "VICTORY_PAGE_WRITES").is_some() {
            log_once("XCL_PAGE_WRITES", &format!("mode={mode}"));
        }
        mode
    })
}

/// Lean map/unmap: skip redundant split/commit (default on; `XCL_MAP_LEAN=0` every call).
pub fn map_lean() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_MAP_LEAN", "VICTORY_MAP_LEAN").is_none_or(|v| v != "0");
        if env_os("XCL_MAP_LEAN", "VICTORY_MAP_LEAN").is_some_and(|v| v == "0") {
            log_once("XCL_MAP_LEAN", "=0 — full map/unmap/commit each change");
        }
        on
    })
}

/// Prefer view-resident byte counts over host query (default on).
pub fn mapped_by_views() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_MAPPED_BY_VIEWS", "VICTORY_MAPPED_BY_VIEWS").is_none_or(|v| v != "0");
        if env_os("XCL_MAPPED_BY_VIEWS", "VICTORY_MAPPED_BY_VIEWS").is_some_and(|v| v == "0") {
            log_once("XCL_MAPPED_BY_VIEWS", "=0 — ask host for mapped bytes");
        }
        on
    })
}

/// When not using views, count mapped bytes by 4 KiB page (`XCL_MAPPED_BY_PAGE=1`; default off → 64 KiB blocks).
pub fn mapped_by_page() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_MAPPED_BY_PAGE", "VICTORY_MAPPED_BY_PAGE").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_MAPPED_BY_PAGE", "1 — mapped-byte probe by 4 KiB page");
        }
        on
    })
}

/// Open only the written view on a fault (default on; `=0` opens all views of the page).
pub fn open_one_view() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_OPEN_ONE_VIEW", "VICTORY_OPEN_ONE_VIEW").is_none_or(|v| v != "0");
        if env_os("XCL_OPEN_ONE_VIEW", "VICTORY_OPEN_ONE_VIEW").is_some_and(|v| v == "0") {
            log_once("XCL_OPEN_ONE_VIEW", "=0 — open every view of a written page");
        }
        on
    })
}

/// Touch page/map latches from refresh/boot.
pub fn note_boot() {
    let _ = (
        page_parts(),
        page_writes(),
        map_lean(),
        mapped_by_views(),
        mapped_by_page(),
        open_one_view(),
    );
}
