//! Host-memory admission for texture prefetch and reusable upload storage (CGBJ Phase 12).
//!
//! Lives at the crate root so `gpu::texture` can consult commit gates without importing `hle`
//! (which already depends on `gpu` for upload paths).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

static PRESSURED: AtomicBool = AtomicBool::new(false);
static HEADROOM: AtomicU64 = AtomicU64::new(u64::MAX);
static NEXT_COMMIT_SAMPLE: AtomicU64 = AtomicU64::new(0);
static AVAILABLE_COMMIT: AtomicU64 = AtomicU64::new(u64::MAX);
static RETRY_UPLOAD_AT: AtomicU64 = AtomicU64::new(0);
static STAGING_GROW_BACKOFF_UNTIL: AtomicU64 = AtomicU64::new(0);
static STAGING_LIVE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static STAGING_DEFERRED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `XCL_RESIDENCY=1` enables commit gates and pressure logging (off by default).
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_RESIDENCY").is_ok_and(|v| !(v.trim().is_empty() || v.trim() == "0")))
}

/// Dynamic descriptor ranges return after the queue's submit fence passes (`XCL_RESIDENCY=1` or `XCL_DYNAMIC_SLOTS` set).
pub fn defer_dynamic_slots() -> bool {
    on() || std::env::var_os("XCL_DYNAMIC_SLOTS").is_some()
}

fn memory_status() -> Option<MEMORYSTATUSEX> {
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status)
}

pub fn coarse_ms() -> u64 {
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

pub(crate) fn allocation_error_hresult(hr: i32) {
    const E_OUTOFMEMORY: i32 = 0x8007_000E_u32 as i32;
    if !on() {
        return;
    }
    if hr == E_OUTOFMEMORY {
        RETRY_UPLOAD_AT.fetch_max(coarse_ms().saturating_add(500), Ordering::Relaxed);
    }
}

pub(crate) fn note_staging_deferred() {
    if on() {
        STAGING_DEFERRED.set(true);
    }
}

pub(crate) fn take_staging_deferred() -> bool {
    if !on() {
        return false;
    }
    STAGING_DEFERRED.replace(false)
}

pub(crate) fn upload_refused() -> bool {
    on() && coarse_ms() < RETRY_UPLOAD_AT.load(Ordering::Relaxed)
}

pub(crate) fn set_live_staging_bytes(bytes: u64) {
    STAGING_LIVE.store(bytes, Ordering::Relaxed);
}

pub(crate) fn live_staging_bytes() -> u64 {
    STAGING_LIVE.load(Ordering::Relaxed)
}

fn sample_commit(now: u64) -> u64 {
    if !on() {
        return u64::MAX;
    }
    let next = NEXT_COMMIT_SAMPLE.load(Ordering::Relaxed);
    if now >= next
        && NEXT_COMMIT_SAMPLE
            .compare_exchange(next, now.saturating_add(32), Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        if let Some(status) = memory_status() {
            AVAILABLE_COMMIT.store(status.ullAvailPageFile, Ordering::Relaxed);
        }
    }
    AVAILABLE_COMMIT.load(Ordering::Relaxed)
}

fn has_upload_reserve(commit: u64, bake: bool) -> bool {
    commit >= if bake { 256 * MIB } else { 512 * MIB }
}

fn has_growth_reserve(commit: u64, bake: bool) -> bool {
    commit >= if bake { 256 * MIB } else { 1024 * MIB }
}

pub(crate) fn admit_texture_pass(bake: bool) -> bool {
    if !on() {
        return true;
    }
    let now = coarse_ms();
    if now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) {
        return false;
    }
    has_upload_reserve(sample_commit(now), bake)
}

pub(crate) fn allow_staging_growth(bake: bool) -> bool {
    if !on() {
        return true;
    }
    let now = coarse_ms();
    if now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) || now < STAGING_GROW_BACKOFF_UNTIL.load(Ordering::Relaxed) {
        return false;
    }
    has_growth_reserve(sample_commit(now), bake)
}

pub(crate) fn note_staging_growth_failed() {
    if on() {
        STAGING_GROW_BACKOFF_UNTIL.store(coarse_ms().saturating_add(500), Ordering::Relaxed);
    }
}

pub(crate) fn limited_host_ram() -> bool {
    if !on() {
        return false;
    }
    static LIMITED: OnceLock<bool> = OnceLock::new();
    *LIMITED.get_or_init(|| memory_status().is_some_and(|s| s.ullTotalPhys <= 32 * GIB))
}

fn next_pressure(was: bool, physical: u64, commit: u64) -> bool {
    if was {
        physical < 5 * GIB || commit < 6 * GIB
    } else {
        physical <= 3 * GIB || commit <= 4 * GIB
    }
}

/// Once per second from the submission path (`present::note_execute`).
pub fn refresh() {
    if !on() {
        return;
    }
    let Some(status) = memory_status() else { return };
    let (physical, commit) = (status.ullAvailPhys, status.ullAvailPageFile);
    HEADROOM.store(physical.min(commit), Ordering::Relaxed);
    let was = pressured();
    let pressure = next_pressure(was, physical, commit);
    if pressure != was {
        PRESSURED.store(pressure, Ordering::Relaxed);
        if pressure {
            crate::xlog_warn!(
                "D3D12.X: host memory pressure (XCL_RESIDENCY): limiting texture prefetch and idle upload storage – physical free {} MiB, commit headroom {} MiB, staging pool {} MiB",
                physical >> 20,
                commit >> 20,
                STAGING_LIVE.load(Ordering::Relaxed) >> 20,
            );
        } else {
            crate::xlog_info!(
                "D3D12.X: host memory pressure cleared (physical free {} MiB, commit headroom {} MiB)",
                physical >> 20,
                commit >> 20,
            );
        }
    }
}

pub(crate) fn headroom() -> Option<u64> {
    if !on() {
        return None;
    }
    let bytes = HEADROOM.load(Ordering::Relaxed);
    (bytes != u64::MAX).then_some(bytes)
}

pub(crate) fn pressured() -> bool {
    on() && PRESSURED.load(Ordering::Relaxed)
}

pub(crate) fn critical_commit() -> bool {
    if !on() {
        return false;
    }
    let now = coarse_ms();
    now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) || sample_commit(now) < 1536 * MIB
}

pub(crate) fn prefetch_limit(bytes: u64, pressure: bool, critical: bool, limited: bool, staging: u64) -> u64 {
    if !on() {
        return bytes;
    }
    if critical {
        bytes.min(16 << 20)
    } else if pressure || (limited && staging >= GIB) {
        bytes.min(64 << 20)
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_and_lookahead_leave_room_for_live_game_allocations() {
        for (commit, bake, lookahead, growth) in [
            (128 * MIB, false, false, false),
            (256 * MIB, true, false, false),
            (512 * MIB, true, true, false),
            (768 * MIB, true, true, false),
            (1024 * MIB, true, true, true),
        ] {
            assert_eq!(has_upload_reserve(commit, true), bake);
            assert_eq!(has_upload_reserve(commit, false), lookahead);
            assert_eq!(has_growth_reserve(commit, false), growth);
        }
    }

    #[test]
    fn either_constraint_enters_pressure_and_both_must_recover() {
        assert!(next_pressure(false, 12 * GIB, 4 * GIB));
        assert!(next_pressure(false, 3 * GIB, 16 * GIB));
        assert!(next_pressure(true, 4 * GIB, 7 * GIB));
        assert!(next_pressure(true, 6 * GIB, 5 * GIB));
        assert!(!next_pressure(true, 5 * GIB, 6 * GIB));
        assert!(!next_pressure(false, 4 * GIB, 5 * GIB));
    }

    #[test]
    fn prefetch_respects_existing_budgets_and_resumes_after_recovery() {
        assert_eq!(prefetch_limit(512 << 20, false, false, true, GIB), 64 << 20);
        assert_eq!(prefetch_limit(512 << 20, false, false, true, GIB - 1), 512 << 20);
        assert_eq!(prefetch_limit(512 << 20, false, false, false, 2 * GIB), 512 << 20);
        assert_eq!(prefetch_limit(0, true, true, false, 0), 0);
        assert_eq!(prefetch_limit(8 << 20, true, true, false, 0), 8 << 20);
        assert_eq!(prefetch_limit(512 << 20, true, false, false, 0), 64 << 20);
        assert_eq!(prefetch_limit(512 << 20, true, true, false, 0), 16 << 20);
        assert_eq!(prefetch_limit(192 << 20, false, false, false, 0), 192 << 20);
    }
}