//! Remaining ABSENT boot/render/texture/shader/window latches.
//!
//! CGBJ `VICTORY_*` parity; GPL-2 reimplementation (study-only @ 80e8619).
//! Prefer these over sprawling call-site rewrites; wire from refresh/boot paths.
#![allow(dead_code)] // scrape-parity latches; call sites adopt gradually


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

fn default_on(xcl: &'static str, victory: &'static str, detail_on: &str) -> bool {
    let on = env_os(xcl, victory).is_none_or(|v| v != "0");
    if env_os(xcl, victory).is_some_and(|v| v == "0") {
        log_once(xcl, "=0 — off");
    } else if env_os(xcl, victory).is_some() {
        log_once(xcl, detail_on);
    }
    on
}

fn opt_in_1(xcl: &'static str, victory: &'static str, detail: &str) -> bool {
    let on = env_str(xcl, victory).is_some_and(|v| v.trim() == "1");
    if on {
        log_once(xcl, detail);
    }
    on
}

fn opt_set(xcl: &'static str, victory: &'static str, detail: &str) -> bool {
    let on = env_os(xcl, victory).is_some_and(|v| !v.is_empty() && v != "0");
    if on {
        log_once(xcl, detail);
    }
    on
}



/// `XCL_TEXTURE_KEEP` (default on; `=0` off).
pub fn texture_keep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_KEEP", "VICTORY_TEXTURE_KEEP", "retain idle textures with longer reclaim times"))
}


/// `XCL_TEXTURE_HOT` (default on; `=0` off).
pub fn texture_hot() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_HOT", "VICTORY_TEXTURE_HOT", "poll hot textures every frame"))
}


/// `XCL_TEXTURE_ONCE` (default on; `=0` off).
pub fn texture_once() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_ONCE", "VICTORY_TEXTURE_ONCE", "skip guard re-ask when upload buffer retires"))
}


/// `XCL_TEXTURE_SAME` (default on; `=0` off).
pub fn texture_same() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_SAME", "VICTORY_TEXTURE_SAME", "reuse unchanged host texture copies"))
}


/// `XCL_TEXTURE_SWEEP` (default on; `=0` off).
pub fn texture_sweep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_SWEEP", "VICTORY_TEXTURE_SWEEP", "sweep idle textures under VRAM pressure"))
}


/// `XCL_TEXTURE_GUARD` (default on; `=0` off).
pub fn texture_guard() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_GUARD", "VICTORY_TEXTURE_GUARD", "hash-guard texture blocks between checks"))
}


/// `XCL_TEXTURE_ROOM_PERCENT` (default 90).
pub fn texture_room_percent() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_TEXTURE_ROOM_PERCENT", "VICTORY_TEXTURE_ROOM_PERCENT")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(90);
        if env_os("XCL_TEXTURE_ROOM_PERCENT", "VICTORY_TEXTURE_ROOM_PERCENT").is_some() {
            log_once("XCL_TEXTURE_ROOM_PERCENT", &format!("{n}"));
        }
        n
    })
}


/// `XCL_TEXTURE_STAGE_MB` optional integer: per-frame staging budget MiB.
pub fn texture_stage_mb() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_TEXTURE_STAGE_MB", "VICTORY_TEXTURE_STAGE_MB").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_TEXTURE_STAGE_MB", &format!("{n} — per-frame staging budget MiB"));
        }
        n
    })
}


/// `XCL_TEXTURE_STAGE_MS` (default 250).
pub fn texture_stage_ms() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_TEXTURE_STAGE_MS", "VICTORY_TEXTURE_STAGE_MS")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(250);
        if env_os("XCL_TEXTURE_STAGE_MS", "VICTORY_TEXTURE_STAGE_MS").is_some() {
            log_once("XCL_TEXTURE_STAGE_MS", &format!("{n}"));
        }
        n
    })
}


/// `XCL_VIEW_BYTES` (default on; `=0` off).
pub fn view_bytes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_BYTES", "VICTORY_VIEW_BYTES", "materialize buffer view bytes from host copies"))
}


/// `XCL_VIEW_COPY` (default on; `=0` off).
pub fn view_copy() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_COPY", "VICTORY_VIEW_COPY", "stage descriptor views off shared heaps"))
}


/// `XCL_VIEW_EXTENT` (default on; `=0` off).
pub fn view_extent() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_EXTENT", "VICTORY_VIEW_EXTENT", "clamp views to mapped extents"))
}


/// `XCL_VIEW_GUARD` (default on; `=0` off).
pub fn view_guard() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_GUARD", "VICTORY_VIEW_GUARD", "guard view memory between publishes"))
}


/// `XCL_VIEW_INLINE` (default on; `=0` off).
pub fn view_inline() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_VIEW_INLINE", "VICTORY_VIEW_INLINE", "inline small view records"))
}


/// `XCL_VIEW_LIMIT` optional integer: cap concurrent view materializations.
pub fn view_limit() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_VIEW_LIMIT", "VICTORY_VIEW_LIMIT").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_VIEW_LIMIT", &format!("{n} — cap concurrent view materializations"));
        }
        n
    })
}


/// `XCL_VIEW_THREADS` optional integer: parallel view land threads.
pub fn view_threads() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_VIEW_THREADS", "VICTORY_VIEW_THREADS").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_VIEW_THREADS", &format!("{n} — parallel view land threads"));
        }
        n
    })
}


/// `XCL_WIDE_FILLS` (default on; `=0` off).
pub fn wide_fills() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WIDE_FILLS", "VICTORY_WIDE_FILLS", "parallel fill of wide host copies"))
}


/// `XCL_WIDE_MERGE` (default on; `=0` off).
pub fn wide_merge() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WIDE_MERGE", "VICTORY_WIDE_MERGE", "merge wide host-copy work"))
}


/// `XCL_WIDE_REFRESH` (default on; `=0` off).
pub fn wide_refresh() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WIDE_REFRESH", "VICTORY_WIDE_REFRESH", "refresh host copies on helper threads"))
}


/// `XCL_WIDE_SPIN` (default on; `=0` off).
pub fn wide_spin() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WIDE_SPIN", "VICTORY_WIDE_SPIN", "spin while wide windows drain"))
}


/// `XCL_WIDE_TEXTURES` (default on; `=0` off).
pub fn wide_textures() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WIDE_TEXTURES", "VICTORY_WIDE_TEXTURES", "upload textures on helper threads"))
}


/// `XCL_UPLOAD_LOW` (default on; `=0` off).
pub fn upload_low() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_UPLOAD_LOW", "VICTORY_UPLOAD_LOW", "prefer low-priority upload heap"))
}


/// `XCL_UPCOPY_CHECK=1`: check up-copies against title memory.
pub fn upcopy_check() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_UPCOPY_CHECK", "VICTORY_UPCOPY_CHECK", "1 — check up-copies against title memory"))
}


/// `XCL_SCRATCH_POOL` (default on; `=0` off).
pub fn scratch_pool() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SCRATCH_POOL", "VICTORY_SCRATCH_POOL", "pool scratch chunks across lists"))
}


/// `XCL_SCRATCH_SPARE_MB` optional integer: spare scratch pool MiB.
pub fn scratch_spare_mb() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_SCRATCH_SPARE_MB", "VICTORY_SCRATCH_SPARE_MB").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_SCRATCH_SPARE_MB", &format!("{n} — spare scratch pool MiB"));
        }
        n
    })
}


/// `XCL_LIGHT_REFRESH` (default on; `=0` off).
pub fn light_refresh() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LIGHT_REFRESH", "VICTORY_LIGHT_REFRESH", "skip refresh of unchanged published views"))
}


/// `XCL_UNWATCHED_FRAMES` (default 2).
pub fn unwatched_frames() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_UNWATCHED_FRAMES", "VICTORY_UNWATCHED_FRAMES")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(2);
        if env_os("XCL_UNWATCHED_FRAMES", "VICTORY_UNWATCHED_FRAMES").is_some() {
            log_once("XCL_UNWATCHED_FRAMES", &format!("{n}"));
        }
        n
    })
}


/// `XCL_WRITE_SKIP` (default on; `=0` off).
pub fn write_skip() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WRITE_SKIP", "VICTORY_WRITE_SKIP", "skip redundant title write sites"))
}

/// `XCL_WAIT_RESIDENT` (default on; `=0` off).
pub fn wait_resident() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WAIT_RESIDENT", "VICTORY_WAIT_RESIDENT", "wait until new card memory is mapped"))
}


/// `XCL_WAIT_ASK=1`: ask before meeting list waits.
pub fn wait_ask() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WAIT_ASK", "VICTORY_WAIT_ASK", "1 — ask before meeting list waits"))
}


/// `XCL_WAIT_FORCE` (default on; `=0` off).
pub fn wait_force() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WAIT_FORCE", "VICTORY_WAIT_FORCE", "drop cross-frame wait holds after present"))
}


/// `XCL_SPLIT_WAITS` (default on; `=0` off).
pub fn split_waits() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SPLIT_WAITS", "VICTORY_SPLIT_WAITS", "split submission waits per list"))
}


/// `XCL_CAPTURE` path: frame capture output folder.
pub fn capture() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_CAPTURE", "VICTORY_CAPTURE").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_CAPTURE", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_RAYS=1`: translate / run ray pipelines.
pub fn rays() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_RAYS", "VICTORY_RAYS", "1 — translate / run ray pipelines"))
}


/// `XCL_D3D12_DRED` set: enable device-removed data / breadcrumbs.
pub fn d3d12_dred() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_D3D12_DRED", "VICTORY_D3D12_DRED", "enable device-removed data / breadcrumbs"))
}


/// `XCL_BOOK` (default on; `=0` off).
pub fn book() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_BOOK", "VICTORY_BOOK", "keep descriptor book with DRED"))
}


/// `XCL_SLOT_CHECK` (default on; `=0` off).
pub fn slot_check() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SLOT_CHECK", "VICTORY_SLOT_CHECK", "check descriptor slot bookkeeping"))
}


/// `XCL_SLOT_HOLDS` (default on; `=0` off).
pub fn slot_holds() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SLOT_HOLDS", "VICTORY_SLOT_HOLDS", "hold slots while views stay live"))
}


/// `XCL_INPLACE` set: enable in-place title memory path.
pub fn inplace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_INPLACE", "VICTORY_INPLACE", "enable in-place title memory path"))
}


/// `XCL_INPLACE_MB` optional integer: in-place path size threshold MiB.
pub fn inplace_mb() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_INPLACE_MB", "VICTORY_INPLACE_MB").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_INPLACE_MB", &format!("{n} — in-place path size threshold MiB"));
        }
        n
    })
}


/// `XCL_INPLACE_PROBE` set: probe which title memory can open in-place.
pub fn inplace_probe() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_INPLACE_PROBE", "VICTORY_INPLACE_PROBE", "probe which title memory can open in-place"))
}


/// `XCL_INPLACE_PROBE_MB` optional integer: in-place probe size MiB.
pub fn inplace_probe_mb() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_INPLACE_PROBE_MB", "VICTORY_INPLACE_PROBE_MB").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_INPLACE_PROBE_MB", &format!("{n} — in-place probe size MiB"));
        }
        n
    })
}


/// `XCL_PICKED_MB` optional integer: picked-texture budget MiB.
pub fn picked_mb() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_PICKED_MB", "VICTORY_PICKED_MB").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_PICKED_MB", &format!("{n} — picked-texture budget MiB"));
        }
        n
    })
}


/// `XCL_PICKED_WRITES` (default on; `=0` off).
pub fn picked_writes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PICKED_WRITES", "VICTORY_PICKED_WRITES", "track writes to picked textures"))
}


/// `XCL_PLACED_FILLER` (default on; `=0` off).
pub fn placed_filler() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PLACED_FILLER", "VICTORY_PLACED_FILLER", "fill placed resource gaps"))
}


/// `XCL_PLACED_ORDER` (default on; `=0` off).
pub fn placed_order() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PLACED_ORDER", "VICTORY_PLACED_ORDER", "order placed resource steps"))
}


/// `XCL_QUICK_FIND` (default on; `=0` off).
pub fn quick_find() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_QUICK_FIND", "VICTORY_QUICK_FIND", "fast shadow window lookup"))
}


/// `XCL_QUICK_MIRROR` (default on; `=0` off).
pub fn quick_mirror() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_QUICK_MIRROR", "VICTORY_QUICK_MIRROR", "mirror descriptors without full pass"))
}


/// `XCL_RAW_VIEWS` (default on; `=0` off).
pub fn raw_views() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_RAW_VIEWS", "VICTORY_RAW_VIEWS", "emit raw twins of buffer views"))
}


/// `XCL_SCREEN_WAIT_MS` (default 10000).
pub fn screen_wait_ms() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_SCREEN_WAIT_MS", "VICTORY_SCREEN_WAIT_MS")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(10000);
        if env_os("XCL_SCREEN_WAIT_MS", "VICTORY_SCREEN_WAIT_MS").is_some() {
            log_once("XCL_SCREEN_WAIT_MS", &format!("{n}"));
        }
        n
    })
}


/// `XCL_SNAPSHOT_POOL` (default on; `=0` off).
pub fn snapshot_pool() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SNAPSHOT_POOL", "VICTORY_SNAPSHOT_POOL", "pool shadow snapshots"))
}


/// `XCL_STORE_BESIDE` (default on; `=0` off).
pub fn store_beside() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_STORE_BESIDE", "VICTORY_STORE_BESIDE", "store title writes beside recording"))
}


/// `XCL_SAME_RECORDS` (default on; `=0` off).
pub fn same_records() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SAME_RECORDS", "VICTORY_SAME_RECORDS", "reuse identical title-write records"))
}


/// `XCL_STREAM_PRIORITY` (default on; `=0` off).
pub fn stream_priority() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_STREAM_PRIORITY", "VICTORY_STREAM_PRIORITY", "lift stream threads above normal"))
}


/// `XCL_STREAM_NOTICE` (default on; `=0` off).
pub fn stream_notice() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_STREAM_NOTICE", "VICTORY_STREAM_NOTICE", "log stream priority lifts"))
}


/// `XCL_SWEEP` (default on; `=0` off).
pub fn sweep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SWEEP", "VICTORY_SWEEP", "sweep idle host resources"))
}


/// `XCL_BURY_LAG` (default on; `=0` off).
pub fn bury_lag() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_BURY_LAG", "VICTORY_BURY_LAG", "bury lagged submissions"))
}


/// `XCL_TRAIL` (default on; `=0` off).
pub fn trail() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TRAIL", "VICTORY_TRAIL", "keep submission trail diagnostics"))
}


/// `XCL_PANEL` set: show host panel overlay.
pub fn panel() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_PANEL", "VICTORY_PANEL", "show host panel overlay"))
}


/// `XCL_MEMORY_DUMP` set: dump GPU memory maps.
pub fn memory_dump() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_MEMORY_DUMP", "VICTORY_MEMORY_DUMP", "dump GPU memory maps"))
}


/// `XCL_MEMORY_MAP` set: log memory map transitions.
pub fn memory_map() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_MEMORY_MAP", "VICTORY_MEMORY_MAP", "log memory map transitions"))
}


/// `XCL_MAP_THREADS` optional integer: parallel map threads.
pub fn map_threads() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_MAP_THREADS", "VICTORY_MAP_THREADS").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_MAP_THREADS", &format!("{n} — parallel map threads"));
        }
        n
    })
}


/// `XCL_MERGE_RACE=1`: stress merge race diagnostics.
pub fn merge_race() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_MERGE_RACE", "VICTORY_MERGE_RACE", "1 — stress merge race diagnostics"))
}


/// `XCL_KNOWN_HOLES` (default on; `=0` off).
pub fn known_holes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_KNOWN_HOLES", "VICTORY_KNOWN_HOLES", "track known guard holes"))
}


/// `XCL_THIN_THREADS` optional integer: thin-list worker threads.
pub fn thin_threads() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_THIN_THREADS", "VICTORY_THIN_THREADS").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_THIN_THREADS", &format!("{n} — thin-list worker threads"));
        }
        n
    })
}


/// `XCL_THREAD_DIMENSIONS` (default on; `=0` off).
pub fn thread_dimensions() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_THREAD_DIMENSIONS", "VICTORY_THREAD_DIMENSIONS", "honor thread-group dimensions"))
}


/// `XCL_UNDER_THREADS` optional integer: under-land worker threads.
pub fn under_threads() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_UNDER_THREADS", "VICTORY_UNDER_THREADS").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_UNDER_THREADS", &format!("{n} — under-land worker threads"));
        }
        n
    })
}


/// `XCL_UNDER_WIDE` (default on; `=0` off).
pub fn under_wide() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_UNDER_WIDE", "VICTORY_UNDER_WIDE", "wide under-copy windows"))
}


/// `XCL_UNDER_WORDS` (default on; `=0` off).
pub fn under_words() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_UNDER_WORDS", "VICTORY_UNDER_WORDS", "word-granularity under copies"))
}


/// `XCL_WINDOWS_BAR_MB` optional integer: card window bar MiB.
pub fn windows_bar_mb() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_WINDOWS_BAR_MB", "VICTORY_WINDOWS_BAR_MB").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_WINDOWS_BAR_MB", &format!("{n} — card window bar MiB"));
        }
        n
    })
}


/// `XCL_WINDOWS_OWN` (default on; `=0` off).
pub fn windows_own() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WINDOWS_OWN", "VICTORY_WINDOWS_OWN", "own card windows per submission"))
}


/// `XCL_WINDOW_IDLE_MS` optional integer: idle ms before window reclaim.
pub fn window_idle_ms() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_WINDOW_IDLE_MS", "VICTORY_WINDOW_IDLE_MS").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_WINDOW_IDLE_MS", &format!("{n} — idle ms before window reclaim"));
        }
        n
    })
}


/// `XCL_WINDOW_KB` optional integer: card window size KiB.
pub fn window_kb() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_WINDOW_KB", "VICTORY_WINDOW_KB").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_WINDOW_KB", &format!("{n} — card window size KiB"));
        }
        n
    })
}


/// `XCL_WINDOW_MAX` optional integer: max concurrent card windows.
pub fn window_max() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_WINDOW_MAX", "VICTORY_WINDOW_MAX").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_WINDOW_MAX", &format!("{n} — max concurrent card windows"));
        }
        n
    })
}


/// `XCL_WINDOW_TAILS` (default on; `=0` off).
pub fn window_tails() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WINDOW_TAILS", "VICTORY_WINDOW_TAILS", "keep window tails for reuse"))
}


/// `XCL_WALLS_TRACE=1`: trace residency walls.
pub fn walls_trace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WALLS_TRACE", "VICTORY_WALLS_TRACE", "1 — trace residency walls"))
}

/// `XCL_MESH_OUTPUT_STRICT` set: refuse mesh outputs that do not fit.
pub fn mesh_output_strict() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_MESH_OUTPUT_STRICT", "VICTORY_MESH_OUTPUT_STRICT", "refuse mesh outputs that do not fit"))
}


/// `XCL_FIRST_HALF=1`: emit first-half wave path.
pub fn first_half() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_FIRST_HALF", "VICTORY_FIRST_HALF", "1 — emit first-half wave path"))
}


/// `XCL_REF_PS` path: reference pixel shader path.
pub fn ref_ps() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_REF_PS", "VICTORY_REF_PS").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_REF_PS", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_REF_VS` path: reference vertex shader path.
pub fn ref_vs() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_REF_VS", "VICTORY_REF_VS").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_REF_VS", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_PC_DXC` path: PC dxc.exe path.
pub fn pc_dxc() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_PC_DXC", "VICTORY_PC_DXC").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_PC_DXC", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_TRANSLATOR` set: force translator mode.
pub fn translator() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_TRANSLATOR", "VICTORY_TRANSLATOR", "force translator mode"))
}


/// `XCL_TRANSLATE_ONLY` set: translate shaders then exit.
pub fn translate_only() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_TRANSLATE_ONLY", "VICTORY_TRANSLATE_ONLY", "translate shaders then exit"))
}


/// `XCL_WAVE_DEEP_SKIPS=1`: deep wave skip lowering.
pub fn wave_deep_skips() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WAVE_DEEP_SKIPS", "VICTORY_WAVE_DEEP_SKIPS", "1 — deep wave skip lowering"))
}


/// `XCL_WAVE_LOCAL=1`: prefer local wave ops.
pub fn wave_local() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WAVE_LOCAL", "VICTORY_WAVE_LOCAL", "1 — prefer local wave ops"))
}


/// `XCL_WAVE_OWN_BITS=1`: own-bits wave lane masks.
pub fn wave_own_bits() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WAVE_OWN_BITS", "VICTORY_WAVE_OWN_BITS", "1 — own-bits wave lane masks"))
}


/// `XCL_WAVE_WHY=1`: log wave lowering rationale.
pub fn wave_why() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WAVE_WHY", "VICTORY_WAVE_WHY", "1 — log wave lowering rationale"))
}


/// `XCL_WAVE_WIDE=1`: force wide wave translation.
pub fn wave_wide() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WAVE_WIDE", "VICTORY_WAVE_WIDE", "1 — force wide wave translation"))
}


/// `XCL_WAVE_WIDE_ONLY=1`: only wide-wave translation.
pub fn wave_wide_only() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WAVE_WIDE_ONLY", "VICTORY_WAVE_WIDE_ONLY", "1 — only wide-wave translation"))
}


/// `XCL_LIBRARY` path: pipeline library file / folder.
pub fn library() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_LIBRARY", "VICTORY_LIBRARY").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_LIBRARY", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}

/// Touch latches once so log-once / scrape literals are live.
pub fn on_refresh_start() {
    let _ = (
        texture_keep(),
        texture_hot(),
        texture_once(),
        texture_same(),
        texture_sweep(),
        texture_guard(),
        texture_room_percent(),
        texture_stage_mb().is_some(),
        texture_stage_ms(),
        view_bytes(),
        view_copy(),
        view_extent(),
    );
    let _ = (
        view_guard(),
        view_inline(),
        view_limit().is_some(),
        view_threads().is_some(),
        wide_fills(),
        wide_merge(),
        wide_refresh(),
        wide_spin(),
        wide_textures(),
        upload_low(),
        upcopy_check(),
        scratch_pool(),
    );
    let _ = (
        scratch_spare_mb().is_some(),
        light_refresh(),
        unwatched_frames(),
        write_skip(),
        wait_resident(),
        wait_ask(),
        wait_force(),
        split_waits(),
        capture().is_some(),
        rays(),
        d3d12_dred(),
        book(),
    );
    let _ = (
        slot_check(),
        slot_holds(),
        inplace(),
        inplace_mb().is_some(),
        inplace_probe(),
        inplace_probe_mb().is_some(),
        picked_mb().is_some(),
        picked_writes(),
        placed_filler(),
        placed_order(),
        quick_find(),
        quick_mirror(),
    );
    let _ = (
        raw_views(),
        screen_wait_ms(),
        snapshot_pool(),
        store_beside(),
        same_records(),
        stream_priority(),
        stream_notice(),
        sweep(),
        bury_lag(),
        trail(),
        panel(),
        memory_dump(),
    );
    let _ = (
        memory_map(),
        map_threads().is_some(),
        merge_race(),
        known_holes(),
        thin_threads().is_some(),
        thread_dimensions(),
        under_threads().is_some(),
        under_wide(),
        under_words(),
        windows_bar_mb().is_some(),
        windows_own(),
        window_idle_ms().is_some(),
    );
    let _ = (
        window_kb().is_some(),
        window_max().is_some(),
        window_tails(),
        walls_trace(),
        mesh_output_strict(),
        first_half(),
        ref_ps().is_some(),
        ref_vs().is_some(),
        pc_dxc().is_some(),
        translator(),
        translate_only(),
        wave_deep_skips(),
    );
    let _ = (
        wave_local(),
        wave_own_bits(),
        wave_why(),
        wave_wide(),
        wave_wide_only(),
        library().is_some(),
    );
}
