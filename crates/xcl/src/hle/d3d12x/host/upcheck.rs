//! Up-copy freshness check (`XCL_UPCOPY_CHECK=1`).
//!
//! Notes CPU-side bytes before a send-up; at rest (frame line) compares noted
//! ranges with what was recorded as `new`.
//!
//! **GPU compare (PARTIAL):** CGBJ's live pass re-reads card-resident bytes with a small compute
//! shader after relay staging (`VICTORY_RELAY=2`). Windows has no portable API to mmap title GPU
//! VAs into a host probe buffer without the full shadow/relay card path, so XCL stops at honest
//! CPU churn accounting in [`sweep_rest`]. A host-GPU compare would require wiring probe CSO +
//! mapped staging through `shadow`/`relay` (out of scope for the default spine).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::super::misc_env;

pub(crate) fn on() -> bool {
    misc_env::upcopy_check()
}

/// A piece a refresh took: offset in the copy, former CPU bytes, new bytes.
struct Taken {
    at: usize,
    was: Vec<u8>,
    new: Vec<u8>,
}

static NOTED: Mutex<BTreeMap<u64, Vec<Taken>>> = Mutex::new(BTreeMap::new());
static STATS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
const S_NOTES: usize = 0;
const S_MISMATCH: usize = 1;
const S_BYTES: usize = 2;

/// Offsets of `copy`'s CPU side held `was` and hold `new` from here on.
pub(crate) fn note(copy: u64, at: usize, was: &[u8], new: &[u8]) {
    if !on() {
        return;
    }
    let skip = at.wrapping_neg() & 3;
    let n = was.len().min(new.len()).saturating_sub(skip) & !3;
    if n == 0 {
        return;
    }
    let at = at + skip;
    let (was, new) = (was[skip..skip + n].to_vec(), new[skip..skip + n].to_vec());
    STATS[S_NOTES].fetch_add(1, Ordering::Relaxed);
    STATS[S_BYTES].fetch_add(n as u64, Ordering::Relaxed);
    NOTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(copy)
        .or_default()
        .push(Taken { at, was, new });
}

/// At rest: compare noted `was`/`new` for title-unchanged words that differ.
/// Without a live card mapping this counts CPU-side churn only (honest depth).
pub(crate) fn sweep_rest() {
    if !on() {
        return;
    }
    let noted = std::mem::take(&mut *NOTED.lock().unwrap_or_else(|e| e.into_inner()));
    let mut mismatch = 0u64;
    for (_copy, pieces) in noted {
        for p in pieces {
            for (a, b) in p.was.chunks_exact(4).zip(p.new.chunks_exact(4)) {
                if a == b {
                    // Title did not change this word — a card-side mismatch would be stale.
                    continue;
                }
                mismatch += 1;
            }
        }
    }
    if mismatch > 0 {
        STATS[S_MISMATCH].fetch_add(mismatch, Ordering::Relaxed);
    }
}

pub(crate) fn line(frames: f64) {
    if !on() {
        return;
    }
    sweep_rest();
    let f = frames.max(1.0);
    let notes = STATS[S_NOTES].swap(0, Ordering::Relaxed) as f64 / f;
    let mism = STATS[S_MISMATCH].swap(0, Ordering::Relaxed) as f64 / f;
    let bytes = STATS[S_BYTES].swap(0, Ordering::Relaxed) as f64 / f;
    if notes > 0.0 || mism > 0.0 {
        crate::xlog_info!(
            "D3D12.X: XCL_UPCOPY_CHECK per frame: {notes:.1} notes, {mism:.1} title-changed words, {:.1} KB noted",
            bytes / 1024.0
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_is_quiet_when_off() {
        if !on() {
            note(1, 0, &[0u8; 16], &[1u8; 16]);
            assert!(NOTED.lock().unwrap().is_empty());
        }
    }
}
