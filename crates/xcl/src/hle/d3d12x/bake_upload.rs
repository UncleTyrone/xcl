//! Decoration / material bake upload timing (`XCL_BAKE_*`, `XCL_EARLY_TEXTURES_MS`).

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

/// First N ms of a run: eager texture index uploads (`XCL_EARLY_TEXTURES_MS`, default 12; `0` = never).
pub fn early_textures_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| env_ms("XCL_EARLY_TEXTURES_MS", 12))
}

/// Bake draw window (`XCL_BAKE_TEXTURES_MS`, default 1500; `0` = off).
pub fn bake_textures_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| env_ms("XCL_BAKE_TEXTURES_MS", 1500))
}

/// `XCL_BAKE_EXACT=0`: bake uploads use current title bytes, not exact-back (default on).
pub fn bake_exact() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_BAKE_EXACT").is_none_or(|v| v != "0"))
}

/// `XCL_EARLY_ALL=1`: kick pipeline compiles at creation.
pub fn early_all() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_EARLY_ALL").is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_BAKE_SYNC=1` with `XCL_RESIDENCY=1`: sync submit on first-upload batches.
pub fn sync_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        super::residency::on()
            && std::env::var("XCL_BAKE_SYNC").is_ok_and(|v| !(v.trim().is_empty() || v.trim() == "0"))
    })
}

pub fn active() -> bool {
    BAKING.get() > 0
}

pub fn with_bake<R>(f: impl FnOnce() -> R) -> R {
    BAKING.set(BAKING.get().saturating_add(1));
    let out = f();
    BAKING.set(BAKING.get().saturating_sub(1));
    out
}

/// Whole-texture first upload in the bake window (ms timers + sync).
pub fn treat_first_as_bake(first: bool) -> bool {
    if !first {
        return false;
    }
    let t0 = {
        static T0: OnceLock<std::time::Instant> = OnceLock::new();
        T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
    };
    let bake_win = bake_textures_ms();
    let early = early_textures_ms();
    let in_early = early > 0 && t0 < early;
    let in_bake = bake_win > 0 && t0 < bake_win;
    (in_early || in_bake) && bake_exact() && (sync_on() || in_bake)
}