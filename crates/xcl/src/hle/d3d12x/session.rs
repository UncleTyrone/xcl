//! Session-length diagnostics (Phase 6): serial-submit GPU waits, group-count clamp
//! telemetry, pipeline-create history for device removal, and VRAM budget helpers.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};


/// Dedicated VRAM (MiB) of the chosen adapter — set once at device init.
static DEDICATED_VRAM_MB: OnceLock<u64> = OnceLock::new();

pub fn set_dedicated_vram_mb(mb: u64) {
    let _ = DEDICATED_VRAM_MB.set(mb.max(1));
}

fn env_mb(names: &[&'static str], default_mb: u64) -> u64 {
    for n in names {
        if let Ok(v) = std::env::var(n) {
            if let Ok(mb) = v.trim().parse::<u64>() {
                if mb > 0 {
                    return mb;
                }
            }
        }
    }
    default_mb
}

/// Scale Kelvaroq 16 GB reference budgets by dedicated VRAM (floor at half the reference).
fn scale_from_16gb(reference_mb: u64) -> u64 {
    let vram = DEDICATED_VRAM_MB.get().copied().unwrap_or(16 * 1024);
    let scaled = reference_mb.saturating_mul(vram) / (16 * 1024);
    scaled.max(reference_mb / 2)
}

/// `XCL_TEXTURE_BUDGET_MB` (default 10240 at 16 GB VRAM, scaled).
pub fn texture_budget_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_mb(&["XCL_TEXTURE_BUDGET_MB", "VICTORY_TEXTURE_BUDGET_MB"], scale_from_16gb(10_240));
        crate::xlog_info!("D3D12.X: texture host/VRAM budget {mb} MiB (XCL_TEXTURE_BUDGET_MB)");
        mb
    })
}

pub fn texture_budget_bytes() -> u64 {
    texture_budget_mb() << 20
}

/// Room for more texture bytes (~10% headroom for targets).
pub fn texture_room(extra_bytes: u64) -> bool {
    let budget = texture_budget_bytes();
    let tenth = budget / 10;
    let cap = budget.saturating_sub(tenth);
    let (pages, _) = super::vram::page_bytes();
    pages as u64 + extra_bytes <= cap
}

/// `XCL_CARD_BUFFERS_MB` (default 2048 at 16 GB, scaled).
pub fn card_buffers_budget_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_mb(&["XCL_CARD_BUFFERS_MB", "VICTORY_CARD_BUFFERS_MB"], scale_from_16gb(2048));
        crate::xlog_info!("D3D12.X: card buffer copy budget {mb} MiB (XCL_CARD_BUFFERS_MB)");
        mb
    })
}

pub fn card_buffers_budget_bytes() -> u64 {
    card_buffers_budget_mb() << 20
}

static CARD_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn card_bytes_charged() -> u64 {
    CARD_BYTES.load(Ordering::Relaxed)
}

pub fn note_card_bytes(bytes: u64) {
    CARD_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

pub fn forget_card_bytes(bytes: u64) {
    let _ = CARD_BYTES.fetch_sub(bytes.min(CARD_BYTES.load(Ordering::Relaxed)), Ordering::Relaxed);
}

pub fn card_buffers_room(bytes: u64) -> bool {
    if !card_buffers_enabled() {
        return false;
    }
    let budget = card_buffers_budget_bytes();
    let tenth = budget / 10;
    let cap = budget.saturating_sub(tenth);
    card_bytes_charged().saturating_add(bytes) <= cap
}

/// Card buffer copies enabled (`XCL_CARD_BUFFERS`, default on; `=0` off).
pub fn card_buffers_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_CARD_BUFFERS").is_none_or(|v| v != "0"))
}

/// `XCL_CARD_BUFFERS_MIN_MB` minimum card-copy buffer size.
pub fn card_buffers_min_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| env_mb(&["XCL_CARD_BUFFERS_MIN_MB", "VICTORY_CARD_BUFFERS_MIN_MB"], 0))
}

/// `XCL_CARD_WINDOWS_MB` extra UMA windows (default 0).
pub fn card_windows_bytes() -> u64 {
    static B: OnceLock<u64> = OnceLock::new();
    *B.get_or_init(|| env_mb(&["XCL_CARD_WINDOWS_MB", "VICTORY_CARD_WINDOWS_MB"], 0) << 20)
}

/// `XCL_CARD_ROOM_FOR_BUFFERS` (default on unless `=0`).
pub fn card_room_for_buffers() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_CARD_ROOM_FOR_BUFFERS").is_none_or(|v| v != "0"))
}

/// `XCL_CARD_ROOM_FOR_TARGETS_MB` headroom for render targets.
pub fn card_room_for_targets_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| env_mb(&["XCL_CARD_ROOM_FOR_TARGETS_MB", "VICTORY_CARD_ROOM_FOR_TARGETS_MB"], 3072))
}

// ---------------------------------------------------------------------------------------
// Group-count clamps (ExecuteIndirect / Dispatch / DispatchMesh)
// ---------------------------------------------------------------------------------------

static CLAMP_TOTAL: AtomicU64 = AtomicU64::new(0);
static CLAMP_WINDOW: std::sync::LazyLock<Mutex<(Instant, u64)>> = std::sync::LazyLock::new(|| Mutex::new((Instant::now(), 0)));

/// A group count was folded or clamped — aggregate log at most once per second.
pub fn note_group_clamp(kind: &'static str, before: [u32; 3], after: [u32; 3]) {
    let n = CLAMP_TOTAL.fetch_add(1, Ordering::Relaxed) + 1;
    let mut w = CLAMP_WINDOW.lock().unwrap();
    if w.0.elapsed() >= Duration::from_secs(1) {
        if w.1 > 0 || n == 1 {
            crate::xlog_warn!(
                "D3D12.X: {kind} group clamps in the last second: {} (latest {before:?} -> {after:?}; {n} total)",
                w.1,
            );
        }
        *w = (Instant::now(), 0);
    }
    w.1 += 1;
    if n <= 8 {
        crate::xlog_warn!("D3D12.X: {kind} groups clamped {before:?} -> {after:?}");
    }
}

// ---------------------------------------------------------------------------------------
// Draws while the bound pipeline is still building (stand-in)
// ---------------------------------------------------------------------------------------

static NOT_READY_DRAWS: AtomicU64 = AtomicU64::new(0);
static NOT_READY_WINDOW: std::sync::LazyLock<Mutex<(Instant, u64)>> = std::sync::LazyLock::new(|| Mutex::new((Instant::now(), 0)));

pub fn note_draw_pipeline_not_ready() {
    NOT_READY_DRAWS.fetch_add(1, Ordering::Relaxed);
    let mut w = NOT_READY_WINDOW.lock().unwrap();
    w.1 += 1;
    if w.0.elapsed() >= Duration::from_secs(1) {
        if w.1 > 0 {
            crate::xlog_info!(
                "D3D12.X: draws with a pipeline still building in the last second: {} ({} total)",
                w.1,
                NOT_READY_DRAWS.load(Ordering::Relaxed),
            );
        }
        *w = (Instant::now(), 0);
    }
}

// ---------------------------------------------------------------------------------------
// Pipeline creates (removal one-liner)
// ---------------------------------------------------------------------------------------

static CREATES: Mutex<VecDeque<Instant>> = Mutex::new(VecDeque::new());
static LAST_SUBMIT_DRAWS: AtomicU64 = AtomicU64::new(0);

pub fn note_pipeline_create() {
    let now = Instant::now();
    let mut q = CREATES.lock().unwrap();
    q.push_back(now);
    while q.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(16)) {
        q.pop_front();
    }
}

pub fn pipeline_creates_last_16s() -> usize {
    let now = Instant::now();
    CREATES.lock().unwrap().iter().filter(|t| now.duration_since(**t) <= Duration::from_secs(16)).count()
}

pub fn note_submit_draw_work(n: u64) {
    if n > 0 {
        LAST_SUBMIT_DRAWS.store(n, Ordering::Relaxed);
    }
}

pub fn last_submit_draw_work() -> u64 {
    LAST_SUBMIT_DRAWS.load(Ordering::Relaxed)
}

/// One summary line for `log_device_removed`.
pub fn removal_summary_line() -> String {
    let creates = pipeline_creates_last_16s();
    let (commit, limit) = crate::host::system_commit();
    let commit_mb = commit >> 20;
    let limit_mb = if limit != 0 { format!("{} MB", limit >> 20) } else { "unknown".into() };
    let draws = last_submit_draw_work();
    format!(
        "removal context: {creates} pipeline creates in the last 16 s; process commit {commit_mb} of {limit_mb}; last submit {draws} console draws/dispatches recorded",
    )
}

// ---------------------------------------------------------------------------------------
// Serial submit: one list + GPU fence wait (impl in `late_wait`)
// ---------------------------------------------------------------------------------------

/// After each command list when `XCL_SUBMIT_SERIAL` is on: wait until the queue finishes it.
pub unsafe fn serial_wait_after_submit(queue: *mut c_void) {
    super::late_wait::serial_wait_after_submit(queue);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_budgets_from_16gb_reference() {
        set_dedicated_vram_mb(8192);
        assert_eq!(scale_from_16gb(10_240), 5120);
        assert_eq!(scale_from_16gb(2048), 1024);
    }
}