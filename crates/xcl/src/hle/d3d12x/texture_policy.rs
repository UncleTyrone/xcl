//! Texture upload visibility and pacing (`XCL_SHOW_AFTER`, `XCL_UPLOAD_AHEAD`,
//! `XCL_TEXTURE_NO_WAIT`, `XCL_LEVELS_ON_DEMAND`). Defaults on this tree match the NVIDIA
//! residency bundle unless an env var says otherwise.

use std::sync::OnceLock;

fn env_on(name: &'static str) -> Option<bool> {
    std::env::var(name).ok().map(|v| {
        let s = v.trim();
        !(s.is_empty() || s == "0")
    })
}

fn latch(cell: &OnceLock<bool>, name: &'static str, default_on: bool) -> bool {
    *cell.get_or_init(|| {
        let on = env_on(name).unwrap_or(default_on);
        if on {
            static LOGGED: OnceLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
            let set = LOGGED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
            if let Ok(mut g) = set.lock() {
                if g.insert(name) {
                    crate::xlog_info!("D3D12.X: {name}=1");
                }
            }
        }
        on
    })
}

/// A texture is only sampled after its copy-queue upload has finished (`XCL_SHOW_AFTER`; on).
pub(super) fn show_after() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_SHOW_AFTER", true)
}

/// Proxy latches owned by [`super::texture_env`] (touched from refresh / residency paths).
pub(super) fn texture_away_ms() -> u64 {
    super::texture_env::texture_away_ms()
}

pub(super) fn stale_rule() -> super::texture_env::StaleRule {
    super::texture_env::stale_textures()
}

pub(super) fn tabled_textures_keep() -> bool {
    super::texture_env::tabled_textures_keep()
}

/// First uploads of pending textures may start before the next submit (`XCL_UPLOAD_AHEAD`; off).
pub(super) fn upload_ahead() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_UPLOAD_AHEAD", false)
}

/// A refresh that cannot read title memory keeps the PC texture (`XCL_TEXTURE_NO_WAIT`; on).
/// The first upload of a texture still waits for readable pages.
pub(super) fn texture_no_wait() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_TEXTURE_NO_WAIT", true)
}

/// Placed sampled textures take card memory level by level (`XCL_LEVELS_ON_DEMAND`; off).
pub(super) fn levels_on_demand() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_LEVELS_ON_DEMAND", false)
}

/// Host-copy from the first viewed mip (`XCL_HELD_LEVELS`; default on). See [`super::diag_env::held_levels`].
pub(super) fn held_levels() -> bool {
    super::diag_env::held_levels()
}

/// Host copies deferred until first use (`XCL_LAZY_TEXTURES`; default on).
pub(super) fn lazy_textures() -> bool {
    super::render_env::lazy_textures()
}

/// Since the last diagnostics line: uploads skipped while title memory was unstable ([`texture_no_wait`]).
pub(super) static NO_WAIT_SKIPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Title memory at `src` for `size` bytes looks stable enough to read for a non-first upload.
pub(super) unsafe fn memory_stable_for_read(src: usize, size: usize) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    if size == 0 {
        return true;
    }
    if crate::gpu::uma::get().is_some_and(|u| u.in_committed_window(src, size)) {
        return true;
    }
    if crate::gpu::shader::readable(src, size.min(4096)) {
        return true;
    }
    let samples = [src, src.saturating_add(size / 2), src.saturating_add(size.saturating_sub(1))];
    for &p in &samples {
        let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        if VirtualQuery(p as *const c_void, &mut m, std::mem::size_of_val(&m)) == 0 || m.State != MEM_COMMIT {
            return false;
        }
    }
    true
}

use std::ffi::c_void;