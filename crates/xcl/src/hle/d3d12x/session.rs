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
    super::send_env::on_send_path();
    let mut budget = card_buffers_budget_bytes();
    if let Some(mb) = super::send_env::send_spare_mb() {
        budget = budget.min(mb.saturating_mul(1 << 20).max(1 << 20));
    }
    let _ = (
        super::send_env::send_own(),
        super::send_env::send_bar(),
        super::send_env::send_changed(),
        super::send_env::send_spare_bytes(),
    );
    let tenth = budget / 10;
    let cap = budget.saturating_sub(tenth);
    let room = card_bytes_charged().saturating_add(bytes) <= cap;
    if !room {
        maybe_card_accounting();
    }
    room
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

/// `XCL_CARD_BUFFERS_MIN_KB` (default 64); used when MIN_MB is 0.
pub fn card_buffers_min_kb() -> u64 {
    static KB: OnceLock<u64> = OnceLock::new();
    *KB.get_or_init(|| {
        std::env::var("XCL_CARD_BUFFERS_MIN_KB")
            .or_else(|_| std::env::var("VICTORY_CARD_BUFFERS_MIN_KB"))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(64)
    })
}

pub fn card_buffers_min_bytes() -> u64 {
    let mb = card_buffers_min_mb();
    if mb > 0 {
        mb << 20
    } else {
        card_buffers_min_kb() << 10
    }
}

/// `XCL_CARD_ACCOUNTING=1`: log when card copy budget is tight.
pub fn card_accounting() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("XCL_CARD_ACCOUNTING").is_ok_and(|v| v.trim() == "1")
            || std::env::var("VICTORY_CARD_ACCOUNTING").is_ok_and(|v| v.trim() == "1");
        if on {
            crate::xlog_info!("D3D12.X: XCL_CARD_ACCOUNTING=1");
        }
        on
    })
}

fn maybe_card_accounting() {
    if !card_accounting() {
        return;
    }
    static NEXT_MS: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_millis() as u64)
        .max(1);
    let next = NEXT_MS.load(Ordering::Relaxed);
    if next != 0 && now < next {
        return;
    }
    if NEXT_MS
        .compare_exchange(next, now + 5000, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    crate::xlog_info!(
        "D3D12.X: card copy budget tight ({} B tracked, cap {} B; XCL_CARD_ACCOUNTING)",
        CARD_BYTES.load(Ordering::Relaxed),
        card_buffers_budget_bytes() * 9 / 10
    );
}

/// `XCL_CARD_ALL=1`: every title buffer copy gets PC and card sides (default off).
pub fn card_all() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("XCL_CARD_ALL").is_ok_and(|v| v.trim() == "1")
            || std::env::var("VICTORY_CARD_ALL").is_ok_and(|v| v.trim() == "1");
        if on {
            crate::xlog_info!("D3D12.X: XCL_CARD_ALL=1 — dual-sided card copies for all buffers");
        }
        on
    })
}

/// `XCL_CARD_HASH_LEN=0`: sparse compare covers the whole card buffer.
pub fn card_hash_own_len() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("XCL_CARD_HASH_LEN")
            .or(std::env::var_os("VICTORY_CARD_HASH_LEN"))
            .is_none_or(|v| v != "0")
    })
}

pub fn card_hash_compare_bytes(title_len: u64, card_size: u64) -> u64 {
    if !card_hash_own_len() {
        return card_size;
    }
    title_len.next_multiple_of(16).min(card_size)
}

/// `XCL_CARD_SPANS=1`: stamp GPU time around submissions (diagnostic latch; default off).
pub fn card_spans() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("XCL_CARD_SPANS").is_ok_and(|v| v.trim() == "1")
            || std::env::var("VICTORY_CARD_SPANS").is_ok_and(|v| v.trim() == "1");
        if on {
            crate::xlog_info!("D3D12.X: XCL_CARD_SPANS=1");
        }
        on
    })
}

/// `XCL_MEMORY_CARD_MB` budget for title-memory-by-address card sides (default 4096).
pub fn memory_card_budget_bytes() -> u64 {
    static B: OnceLock<u64> = OnceLock::new();
    *B.get_or_init(|| {
        let mb = env_mb(&["XCL_MEMORY_CARD_MB", "VICTORY_MEMORY_CARD_MB"], 4096);
        crate::xlog_info!("D3D12.X: XCL_MEMORY_CARD_MB={mb}");
        mb << 20
    })
}

static MEMORY_CARD_USED: AtomicU64 = AtomicU64::new(0);

/// Charge `len` against the memory-card budget; returns whether it fits.
pub fn memory_card_room(len: u64) -> bool {
    let most = memory_card_budget_bytes();
    MEMORY_CARD_USED.fetch_add(len, Ordering::Relaxed) + len <= most
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