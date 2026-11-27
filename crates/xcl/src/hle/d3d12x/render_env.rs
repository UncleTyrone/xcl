//! Boot/render env latches (CGBJ `VICTORY_*` parity; GPL-2 reimplementation).
//!
//! Defaults match CGBJ: most knobs are on unless `=0`; a few are opt-in (`=1`).

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

fn env_var(xcl: &str, victory: &str) -> Result<String, std::env::VarError> {
    std::env::var(xcl).or_else(|_| std::env::var(victory))
}

/// On unless the named var is explicitly `0` (CGBJ `toggled!` / `is_none_or`).
fn default_on(xcl: &'static str, victory: &'static str, detail_on: &str) -> bool {
    let on = env_os(xcl, victory).is_none_or(|v| v != "0");
    if env_os(xcl, victory).is_some_and(|v| v == "0") {
        log_once(xcl, "=0 — off");
    } else if env_os(xcl, victory).is_some() {
        log_once(xcl, detail_on);
    }
    on
}

/// Off unless the named var is explicitly `1`.
fn opt_in_1(xcl: &'static str, victory: &'static str, detail: &str) -> bool {
    let on = env_var(xcl, victory).is_ok_and(|v| v.trim() == "1");
    if on {
        log_once(xcl, detail);
    }
    on
}

// —— HEAP_* ——

/// Heap buffer-view pass may run beside list recording (`XCL_HEAP_PASS_ASIDE`; default on).
pub fn heap_pass_aside() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_HEAP_PASS_ASIDE", "VICTORY_HEAP_PASS_ASIDE", "heap view pass may run beside recording"))
}

/// Re-check textures picked by heap index after upload (`XCL_HEAP_RECHECK`; default on).
pub fn heap_recheck() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_HEAP_RECHECK", "VICTORY_HEAP_RECHECK", "recheck heap-picked textures after upload"))
}

/// Spread unwatched heap slot rechecks across passes (`XCL_HEAP_SPREAD`; default on).
pub fn heap_spread() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_HEAP_SPREAD", "VICTORY_HEAP_SPREAD", "spread heap slot rechecks across passes"))
}

/// Early heap-sync round while waiting for submit (`XCL_HEAP_SYNC_EARLY=<ms>`, default 4; `0` off).
pub fn heap_sync_early() -> Option<Duration> {
    static EVERY: OnceLock<Option<Duration>> = OnceLock::new();
    *EVERY.get_or_init(|| {
        let ms = env_var("XCL_HEAP_SYNC_EARLY", "VICTORY_HEAP_SYNC_EARLY")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(4);
        if ms == 0 {
            log_once("XCL_HEAP_SYNC_EARLY", "=0 — no early heap-sync rounds");
            return None;
        }
        if env_os("XCL_HEAP_SYNC_EARLY", "VICTORY_HEAP_SYNC_EARLY").is_some() {
            log_once("XCL_HEAP_SYNC_EARLY", &format!("{ms} ms early heap-sync while waiting for submit"));
        }
        Some(Duration::from_millis(ms))
    })
}

/// Upload textures found through heap index slots (`XCL_HEAP_TEXTURES`; default on).
pub fn heap_textures() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_HEAP_TEXTURES", "VICTORY_HEAP_TEXTURES", "heap-index texture upload path on"))
}

// —— LAND_* ——

/// Land pieces through host views while mappings change (`XCL_LAND_ANYTIME`; default on).
pub fn land_anytime() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LAND_ANYTIME", "VICTORY_LAND_ANYTIME", "land through host views while mappings change"))
}

/// Skip re-upload of pages that only echo a prior landing (`XCL_LAND_ECHO`; default on).
pub fn land_echo() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LAND_ECHO", "VICTORY_LAND_ECHO", "skip echo re-upload of landed pages"))
}

/// Drop pieces an earlier fetch already holds (`XCL_LAND_SKIP=1`; default off).
pub fn land_skip() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_LAND_SKIP", "VICTORY_LAND_SKIP", "skip pieces an earlier fetch still holds"))
}

/// Mark copies up to date after a beside landing (`XCL_LAND_TAKEN`; default on).
pub fn land_taken() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LAND_TAKEN", "VICTORY_LAND_TAKEN", "note landed copies as up to date"))
}

// —— LIST_* ——

/// Max idle host lists retained (`XCL_LIST_IDLE_MOST`, default 512, clamp 16..=4096).
pub fn list_idle_most() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_var("XCL_LIST_IDLE_MOST", "VICTORY_LIST_IDLE_MOST")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .map_or(512usize, |n: usize| n.clamp(16, 4096));
        if env_os("XCL_LIST_IDLE_MOST", "VICTORY_LIST_IDLE_MOST").is_some() {
            log_once("XCL_LIST_IDLE_MOST", &format!("idle list pool cap {n}"));
        }
        n
    })
}

/// Idle-list trim window in milliseconds (`XCL_LIST_IDLE_TRIM_S` seconds; default 0 = never).
pub fn list_idle_trim_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| {
        let ms = env_var("XCL_LIST_IDLE_TRIM_S", "VICTORY_LIST_IDLE_TRIM_S")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map_or(0, |s| s.saturating_mul(1000));
        if ms > 0 {
            log_once("XCL_LIST_IDLE_TRIM_S", &format!("trim idle lists after {} s", ms / 1000));
        }
        ms
    })
}

/// Recycle finished host lists (`XCL_LIST_POOL`; default on).
pub fn list_pool() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LIST_POOL", "VICTORY_LIST_POOL", "recycle finished host lists"))
}

/// Pixel-shader hash prefixes whose draws list heap slots (`XCL_LIST_SLOTS`, comma-separated).
pub fn list_slots() -> &'static [String] {
    static LIST: OnceLock<Vec<String>> = OnceLock::new();
    LIST.get_or_init(|| {
        let raw = env_var("XCL_LIST_SLOTS", "VICTORY_LIST_SLOTS").unwrap_or_default();
        let v: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !v.is_empty() {
            log_once("XCL_LIST_SLOTS", &format!("{} shader hash prefix(es)", v.len()));
        }
        v
    })
    .as_slice()
}

/// Cache list-table views once per list (`XCL_LIST_VIEWS`; default on).
pub fn list_views() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LIST_VIEWS", "VICTORY_LIST_VIEWS", "cache descriptor table views per list"))
}

// —— MEMORY / INDIRECT / LATE / LAZY / LENT / LEVELS ——

/// Cap for title-memory shadows kept in dedicated card memory (`XCL_MEMORY_CARD_MB`, default 4096).
pub fn memory_card_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_var("XCL_MEMORY_CARD_MB", "VICTORY_MEMORY_CARD_MB")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(4096u64);
        if env_os("XCL_MEMORY_CARD_MB", "VICTORY_MEMORY_CARD_MB").is_some() {
            log_once("XCL_MEMORY_CARD_MB", &format!("{mb} MiB card memory for by-address shadows"));
        }
        mb
    })
}

pub fn memory_card_bytes() -> u64 {
    memory_card_mb().saturating_mul(1 << 20)
}

/// Keep indirect clamp budgets in card memory (`XCL_INDIRECT_CARD_BUDGET`; default on).
pub fn indirect_card_budget() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        default_on(
            "XCL_INDIRECT_CARD_BUDGET",
            "VICTORY_INDIRECT_CARD_BUDGET",
            "indirect clamp budgets in card memory",
        )
    })
}

/// Allow wide AMD indirect dispatch dimensions (`XCL_INDIRECT_WIDE`; default on → 2^20 on AMD).
pub fn indirect_wide() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_INDIRECT_WIDE", "VICTORY_INDIRECT_WIDE", "wide indirect dispatch dims on AMD"))
}

/// Per-dimension dispatch/group clamp for indirect/direct folds.
pub fn dispatch_dim_cap(amd: bool) -> u32 {
    if amd && indirect_wide() {
        1 << 20
    } else {
        65_535
    }
}

/// Sleep to the next frame tick on late waits (`XCL_LATE_WAITS=1`; default off = catch already-passed tick).
pub fn late_waits() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_LATE_WAITS", "VICTORY_LATE_WAITS", "sleep to next frame tick on late waits"))
}

/// Build list preludes lazily on first need (`XCL_LAZY_PRELUDE`; default on).
pub fn lazy_prelude() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LAZY_PRELUDE", "VICTORY_LAZY_PRELUDE", "lazy host-list preludes"))
}

/// Defer host texture copies until first use (`XCL_LAZY_TEXTURES`; default on).
pub fn lazy_textures() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LAZY_TEXTURES", "VICTORY_LAZY_TEXTURES", "lazy host texture copies"))
}

/// Lend the largest shader-visible view heap to lists without one (`XCL_LENT_HEAP`; default on).
pub fn lent_heap() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LENT_HEAP", "VICTORY_LENT_HEAP", "lend largest view heap to bare lists"))
}

/// Minimum texture size for on-demand level pages (`XCL_LEVELS_MIN_KB`, default 1024 KiB).
pub fn levels_min_kb() -> u64 {
    static KB: OnceLock<u64> = OnceLock::new();
    *KB.get_or_init(|| {
        let kb = env_var("XCL_LEVELS_MIN_KB", "VICTORY_LEVELS_MIN_KB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map_or(1024, |n| n.max(64));
        if env_os("XCL_LEVELS_MIN_KB", "VICTORY_LEVELS_MIN_KB").is_some() {
            log_once("XCL_LEVELS_MIN_KB", &format!("{kb} KiB floor for on-demand levels"));
        }
        kb
    })
}

pub fn levels_min_bytes() -> u64 {
    levels_min_kb().saturating_mul(1024)
}

/// Touch latches that should appear in boot/refresh logs when any related path is hot.
pub fn on_refresh_start() {
    let _ = (
        heap_pass_aside(),
        heap_recheck(),
        heap_spread(),
        heap_textures(),
        lazy_textures(),
        land_echo(),
        land_anytime(),
        land_taken(),
        land_skip(),
        list_pool(),
        list_views(),
        list_idle_most(),
        list_idle_trim_ms(),
        list_slots().len(),
    );
}
