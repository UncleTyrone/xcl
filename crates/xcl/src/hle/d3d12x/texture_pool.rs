//! Staging upload commit gates and idle buffer reclaim (CGBJ `pool.rs` staging track).

use std::sync::OnceLock;

const SPARE: usize = 2;
const STAGING_KEPT: usize = 8;

/// `XCL_TEXTURE_POOL=1` or unset while `XCL_RESIDENCY=1`; `XCL_TEXTURE_POOL=0` always off.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        match std::env::var("XCL_TEXTURE_POOL") {
            Ok(v) if v.trim() == "0" => false,
            Ok(v) if !v.trim().is_empty() && v.trim() != "0" => true,
            _ => super::residency::on(),
        }
    })
}

pub fn now_ms() -> u64 {
    super::residency::coarse_ms()
}

pub fn note_live_bytes(bytes: u64) {
    super::residency::set_live_staging_bytes(bytes);
}

/// Whether a new upload buffer may be committed (speculative vs bake).
pub fn growth_allowed(bake: bool) -> bool {
    if !on() {
        return true;
    }
    super::residency::allow_staging_growth(bake)
}

pub fn note_growth_failed() {
    if on() {
        super::residency::note_staging_growth_failed();
    }
}

/// Once per second with residency refresh: trim idle upload staging under pressure.
pub fn tick(memory: Option<(u64, u64)>) {
    if !on() {
        return;
    }
    let pressure = super::residency::pressured();
    let _keep_spare = memory
        .filter(|(budget, used)| *budget > 0 && u128::from(*used) * 100 < u128::from(*budget) * 85)
        .is_some();
    let (p, b) = super::present::counters();
    let frames = p.max(b);
    let low = super::residency::critical_commit() || pressure || super::vram::commit_low();
    crate::gpu::texture::trim_idle_staging(frames, low);
    note_live_bytes(crate::gpu::texture::staging_live_bytes());
}

/// Cap in bytes for retained in-flight upload staging (`title_textures::in_flight_cap_bytes`).
pub fn free_cap_bytes() -> u64 {
    if !on() {
        return 512 << 20;
    }
    if super::residency::critical_commit() {
        16 << 20
    } else if super::residency::pressured() {
        64 << 20
    } else {
        512 << 20
    }
}

/// Spare empty 64 MiB blocks target when a full block pool is added (`XCL_RIPE_BLOCKS`).
pub fn spare_blocks_target() -> usize {
    if super::residency::pressured() {
        SPARE
    } else {
        STAGING_KEPT
    }
}