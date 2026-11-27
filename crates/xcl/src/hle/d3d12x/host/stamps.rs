//! Submission begin/end stamps for the timeline (`XCL_TIMELINE`).
//!
//! Wall-clock pairs (µs since process epoch) stand in for card timestamps when
//! a query heap is not yet wired; [`timeline_ready`] arms the ring.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const MOST: usize = 1024;

static EPOCH: OnceLock<Instant> = OnceLock::new();
static NEXT: AtomicUsize = AtomicUsize::new(0);
static PAIRS: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

fn now_us() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// Arm the stamp ring before a timeline recording window.
pub(crate) fn timeline_ready() {
    let _ = EPOCH.get_or_init(Instant::now);
    let mut p = PAIRS.lock().unwrap_or_else(|e| e.into_inner());
    p.clear();
    p.resize(MOST, (0, 0));
    NEXT.store(0, Ordering::Relaxed);
}

/// Place of the next submission's stamps while the timeline records.
pub(crate) fn take() -> Option<usize> {
    if !super::timeline::on() {
        return None;
    }
    let at = NEXT.fetch_add(1, Ordering::Relaxed);
    if at >= MOST {
        return None;
    }
    let mut p = PAIRS.lock().unwrap_or_else(|e| e.into_inner());
    if p.len() < MOST {
        p.resize(MOST, (0, 0));
    }
    p[at].0 = now_us();
    Some(at)
}

/// Mark the submission at `at` finished (or the latest open one).
pub(crate) fn finish(at: Option<usize>) {
    let Some(at) = at else { return };
    let mut p = PAIRS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(slot) = p.get_mut(at) {
        slot.1 = now_us();
    }
}

/// By place, begin/end in PC µs (0,0 if missing).
pub(crate) fn timeline_stamps() -> Vec<(u64, u64)> {
    let taken = NEXT.load(Ordering::Relaxed).min(MOST);
    let p = PAIRS.lock().unwrap_or_else(|e| e.into_inner());
    p.iter().take(taken).copied().collect()
}

pub(crate) fn timeline_restart() {
    NEXT.store(0, Ordering::Relaxed);
    let mut p = PAIRS.lock().unwrap_or_else(|e| e.into_inner());
    for slot in p.iter_mut() {
        *slot = (0, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_is_none_when_timeline_off() {
        assert!(take().is_none());
    }
}
