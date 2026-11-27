//! One-shot material / decoration bake uploads use looser commit gates and sync list submit.

use std::cell::Cell;
use std::sync::OnceLock;

thread_local! {
    static BAKING: Cell<u32> = const { Cell::new(0) };
}

fn env_ms(name: &'static str, default: u64) -> u64 {
    match std::env::var(name).ok().and_then(|v| v.trim().parse().ok()) {
        Some(ms) => {
            crate::xlog_info!("D3D12.X: {name}={ms} ms");
            ms
        }
        None => default,
    }
}

pub fn early_textures_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| env_ms("XCL_EARLY_TEXTURES_MS", 12))
}

pub fn bake_textures_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| env_ms("XCL_BAKE_TEXTURES_MS", 1500))
}

pub fn bake_exact() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_BAKE_EXACT").is_none_or(|v| v != "0"))
}

pub fn early_all() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_EARLY_ALL").is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_BAKE_SYNC=1` routes first-upload batches through synchronous queue submit when residency is on.
pub fn sync_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::host_residency::on() && std::env::var("XCL_BAKE_SYNC").is_ok_and(|v| !(v.trim().is_empty() || v.trim() == "0")))
}

pub fn active() -> bool {
    BAKING.get() > 0
}

/// Mark the current thread as running a bake pass (lower commit thresholds in [`crate::host_residency`]).
pub fn with_bake<R>(f: impl FnOnce() -> R) -> R {
    BAKING.set(BAKING.get().saturating_add(1));
    let out = f();
    BAKING.set(BAKING.get().saturating_sub(1));
    out
}

/// A whole-texture first upload is treated like a bake for admission when sync is on.
pub fn treat_first_as_bake(first: bool) -> bool {
    if !first {
        return false;
    }
    let t0 = crate::host_residency::coarse_ms();
    let in_early = early_textures_ms() > 0 && t0 < early_textures_ms();
    let in_bake = bake_textures_ms() > 0 && t0 < bake_textures_ms();
    (in_early || in_bake) && bake_exact() && (sync_on() || in_bake)
}