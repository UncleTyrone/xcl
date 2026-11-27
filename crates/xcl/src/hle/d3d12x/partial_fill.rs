//! Cross-tree PARTIAL → MATCH fills + remaining boot/render ABSENT latches.
//! GPL-2 reimplementation of CGBJ `VICTORY_*` names (study only @ 80e8619).

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

fn default_on(xcl: &'static str, victory: &'static str) -> bool {
    env_os(xcl, victory).is_none_or(|v| v != "0")
}

fn opt_in_1(xcl: &'static str, victory: &'static str) -> bool {
    std::env::var(xcl)
        .or_else(|_| std::env::var(victory))
        .is_ok_and(|v| v.trim() == "1")
}

/// `XCL_BIND_CACHE` (default on; `=0` off).
pub fn bind_cache() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = default_on("XCL_BIND_CACHE", "VICTORY_BIND_CACHE");
        if env_os("XCL_BIND_CACHE", "VICTORY_BIND_CACHE").is_some() {
            log_once("XCL_BIND_CACHE", if on { "on" } else { "=0 — off" });
        }
        on
    })
}

/// `XCL_BVH_BUILDER=1` runs XboxBvhBuilder programs (default off).
pub fn bvh_builder() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = opt_in_1("XCL_BVH_BUILDER", "VICTORY_BVH_BUILDER");
        if on {
            log_once("XCL_BVH_BUILDER", "run XboxBvhBuilder programs");
        }
        on
    })
}

/// `XCL_DUMP_TARGETS=1` keeps RTV/DSV resources for frame dumps.
pub fn dump_targets() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_DUMP_TARGETS", "VICTORY_DUMP_TARGETS"))
}

/// `XCL_FAST_CORES` prefer performance cores for helper threads (default on).
pub fn fast_cores() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_FAST_CORES", "VICTORY_FAST_CORES"))
}

/// `XCL_HEAP_SYNC` mirror descriptor heaps (default on).
pub fn heap_sync() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_HEAP_SYNC", "VICTORY_HEAP_SYNC"))
}

/// `XCL_HEAP_SYNC_ASIDE` sync beside recording (default on when heap_sync).
pub fn heap_sync_aside() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| heap_sync() && default_on("XCL_HEAP_SYNC_ASIDE", "VICTORY_HEAP_SYNC_ASIDE"))
}

/// `XCL_PIPELINE_WAIT_MS` per-second graphics wait refill.
pub fn pipeline_wait_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| {
        std::env::var("XCL_PIPELINE_WAIT_MS")
            .or_else(|_| std::env::var("VICTORY_PIPELINE_WAIT_MS"))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(4)
    })
}

/// `XCL_PORT_CBV_PLACE` place CBV constants into port journal (default on).
pub fn port_cbv_place() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PORT_CBV_PLACE", "VICTORY_PORT_CBV_PLACE"))
}

/// `XCL_RDNA_FLOAT` RDNA float mode latch (default on).
pub fn rdna_float() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_RDNA_FLOAT", "VICTORY_RDNA_FLOAT"))
}

/// `XCL_TEXTURE_BUDGET_MB` host/VRAM texture budget (default 10240 @ 16 GB).
pub fn texture_budget_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| {
        std::env::var("XCL_TEXTURE_BUDGET_MB")
            .or_else(|_| std::env::var("VICTORY_TEXTURE_BUDGET_MB"))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(10_240)
    })
}

/// `XCL_WARP=1` force WARP adapter (alias of D3D12_WARP path).
pub fn warp() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WARP", "VICTORY_WARP"))
}
