//! Staging upload commit gates and idle buffer reclaim (CGBJ `pool.rs` staging track).

use std::sync::OnceLock;

const SPARE: usize = 2;
const STAGING_KEPT: usize = 8;

/// `XCL_TEXTURE_POOL=1` or unset while `XCL_RESIDENCY=1`; `XCL_TEXTURE_POOL=0` always off.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let _ = super::render_env::wait_resident();
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

/// Once per second with residency refresh: reclaim free upload jobs under pressure.
///
/// Nvidia tree keeps jobs in `texture_upload::FREE` rather than `gpu::texture` staging pools;
/// caps match main/amd `texture_pool::free_cap_bytes`.
pub fn tick(memory: Option<(u64, u64)>) {
    if !on() {
        return;
    }
    if !super::texture_env::texture_sweep_on() && !super::render_env::sweep() {
        return;
    }
    let pressure = super::residency::pressured();
    let room = super::texture_env::texture_room_percent();
    let _keep_spare = memory
        .filter(|(budget, used)| *budget > 0 && u128::from(*used) * 100 < u128::from(*budget) * room as u128)
        .is_some();
    let cap = if super::residency::critical_commit() {
        16 << 20
    } else if pressure {
        64 << 20
    } else {
        512 << 20
    };
    super::texture_upload::reclaim_under_cap(cap);
}

/// Cap in bytes for retained free upload jobs (`texture_upload::FREE`).
pub fn free_cap_bytes() -> u64 {
    let _ = super::texture_env::texture_room_percent();
    let _ = super::texture_env::texture_sweep();
    let _ = super::texture_env::texture_keep();
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
