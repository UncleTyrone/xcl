//! Late GPU flights (`XCL_LATE_WAIT`, cool_fox `VICTORY_LATE_WAIT` 0/1/2/3). **Default off.**
//!
//! Behavioral port (GPL-2 clean; no CGBJ paste). Intended overlap with relay + async submit.
//!
//! | Stage | What this tree does today |
//! |-------|---------------------------|
//! | 0 | Unchanged sync path. |
//! | 1 | Skip per-chunk [`super::session::serial_wait_after_submit`] when cap allows. |
//! | 2 | Stage 1 + shorten/no [`super::alloc_pool`] resubmit waits when cap allows. |
//! | 3 | Stage 2 + [`super::submit::present_may_queue`] for frames-ahead present scaffolding. |
//!
//! **Gaps vs CGBJ `host::late`:** no flight object / relay numbering, no `late_met` value-waits,
//! no bottom-of-pipe write settle across flights, present is not its own GPU command-list flight,
//! no per-frame `LATE_WAITED` overlap histogram in the frame log.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

static LOGGED: OnceLock<()> = OnceLock::new();

/// Raw env (ignores `XCL_GPU_TIMING` force-off).
pub fn raw_stage() -> u32 {
    static S: OnceLock<u32> = OnceLock::new();
    *S.get_or_init(|| match std::env::var("XCL_LATE_WAIT").ok().as_deref().map(str::trim) {
        None | Some("") | Some("0") | Some("off") => 0,
        Some("1") => 1,
        Some("2") => 2,
        Some("3") => 3,
        Some(bad) => {
            crate::xlog_warn!("D3D12.X: XCL_LATE_WAIT={bad:?} invalid (use 0–3); treating as off");
            0
        }
    })
}

/// `XCL_GPU_TIMING` forces synchronous timing (late flights off), matching cool_fox.
pub fn gpu_timing_forces_sync() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_GPU_TIMING").is_some())
}

/// Effective stage: 0 when unset, forced 0 with `XCL_GPU_TIMING`.
pub fn stage() -> u32 {
    let s = raw_stage();
    if s == 0 {
        return 0;
    }
    if gpu_timing_forces_sync() {
        return 0;
    }
    LOGGED.get_or_init(|| {
        crate::xlog_info!(
            "D3D12.X: XCL_LATE_WAIT={s} (cap {} in-flight chunks, BOP settle {})",
            max_in_flight_chunks(),
            settle_bottom_writes(),
        );
        if s >= 2 && !matches!(std::env::var("XCL_RELAY").ok().as_deref().map(str::trim), Some("1") | Some("2")) {
            crate::xlog_warn!("D3D12.X: XCL_LATE_WAIT>=2 expects XCL_RELAY=1|2 for land overlap (relay not enabled)");
        }
        if s >= 3 && !super::port::async_submit() {
            crate::xlog_warn!("D3D12.X: XCL_LATE_WAIT=3 pairs with XCL_ASYNC_SUBMIT=1 for present-ahead scaffolding");
        }
    });
    s
}

pub fn on() -> bool {
    stage() > 0
}

pub fn present_ahead() -> bool {
    stage() >= 3
}

/// `XCL_LATE_WAIT_LISTS` (default 256).
pub fn max_in_flight_chunks() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("XCL_LATE_WAIT_LISTS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(256)
            .max(1)
    })
}

/// `XCL_LATE_WAIT_WRITES`: on unless `=0` (stub — no cross-flight BOP settle yet).
pub fn settle_bottom_writes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_LATE_WAIT_WRITES").is_none_or(|v| v != "0"))
}

static CHUNKS_AHEAD: AtomicU64 = AtomicU64::new(0);
static SKIPPED_SERIAL: AtomicU64 = AtomicU64::new(0);
static SHORT_RESUBMIT: AtomicU64 = AtomicU64::new(0);

pub fn chunks_ahead() -> u64 {
    CHUNKS_AHEAD.load(Ordering::Acquire)
}

pub fn may_send_ahead() -> bool {
    on() && chunks_ahead() < max_in_flight_chunks() as u64
}

/// After a driver execute chunk is queued (no serial wait yet).
pub fn note_chunk_sent() {
    if on() {
        CHUNKS_AHEAD.fetch_add(1, Ordering::Relaxed);
    }
}

/// Present or frame boundary: reset in-flight chunk counter (full flight tracking TBD).
pub fn note_present_boundary() {
    CHUNKS_AHEAD.store(0, Ordering::Release);
}

/// When true, AMD relay [`super::relay::wait_before_submit`] may be skipped (stage ≥ 1 under cap).
pub fn relax_relay_wait_before_submit() -> bool {
    stage() >= 1 && may_send_ahead()
}

/// Skip [`super::session::serial_wait_after_submit`] when late flights are on and under cap.
pub fn skip_serial_wait() -> bool {
    if on() && may_send_ahead() {
        SKIPPED_SERIAL.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Override for [`super::alloc_pool::resubmit_wait`]; `None` = use default env/ms.
pub fn resubmit_wait_override() -> Option<Duration> {
    if stage() < 2 || !may_send_ahead() {
        return None;
    }
    SHORT_RESUBMIT.fetch_add(1, Ordering::Relaxed);
    Some(Duration::ZERO)
}

pub fn log_frame_stats(frames: f64) {
    if !on() || frames <= 0.0 {
        return;
    }
    let serial = SKIPPED_SERIAL.swap(0, Ordering::Relaxed) as f64 / frames;
    let resub = SHORT_RESUBMIT.swap(0, Ordering::Relaxed) as f64 / frames;
    if serial > 0.0 || resub > 0.0 {
        crate::xlog_info!(
            "D3D12.X: late-wait XCL_LATE_WAIT={} per frame: {:.2} serial waits skipped, {:.2} short resubmit waits",
            stage(),
            serial,
            resub,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_wait_off_by_default() {
        if std::env::var_os("XCL_LATE_WAIT").is_none() {
            assert_eq!(raw_stage(), 0);
            assert!(!on());
        }
    }

    #[test]
    fn lists_cap_defaults() {
        if std::env::var_os("XCL_LATE_WAIT_LISTS").is_none() {
            assert_eq!(max_in_flight_chunks(), 256);
        }
    }
}