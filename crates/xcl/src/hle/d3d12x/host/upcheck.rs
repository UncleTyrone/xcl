//! Up-copy freshness check (`XCL_UPCOPY_CHECK=1`).
//!
//! Notes CPU-side bytes before a send-up; at rest (frame line) compares noted
//! ranges with what was recorded as `new`.
//!
//! **GPU compare (PARTIAL):** CGBJ's live pass re-reads card-resident bytes with a small compute
//! shader after relay staging (`VICTORY_RELAY=2`). XCL does not ship the probe CSO; when shadow
//! copies exist, [`sweep_rest`] compares noted `was` against post-land `held` for title-stable words
//! (honest proxy for card bytes after `relay_finish_ranges`). Full per-submit GPU compare remains out
//! of scope without the CGBJ dispatch path.

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
static STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
const S_NOTES: usize = 0;
const S_MISMATCH: usize = 1;
const S_BYTES: usize = 2;
const S_STALE: usize = 3;

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

/// At rest: title-changed words (`was` ≠ `new`) vs stale sends on title-stable words (`was` == `new`
/// but `held` ≠ `was` when a shadow exists).
pub(crate) fn sweep_rest() {
    if !on() {
        return;
    }
    let noted = std::mem::take(&mut *NOTED.lock().unwrap_or_else(|e| e.into_inner()));
    let mut title_changed = 0u64;
    let mut stale = 0u64;
    for (copy, pieces) in noted {
        let shadow = super::super::shadow::by_copy_id(copy);
        for p in pieces {
            let words = p.was.len().min(p.new.len()) / 4;
            for wi in 0..words {
                let a = wi * 4;
                let (was, new) = (&p.was[a..a + 4], &p.new[a..a + 4]);
                if was != new {
                    title_changed += 1;
                    continue;
                }
                let Some(ref s) = shadow else { continue };
                let Some(held) = s.held_span(p.at + a, 4) else { continue };
                if held != was {
                    stale += 1;
                }
            }
        }
    }
    if title_changed > 0 {
        STATS[S_MISMATCH].fetch_add(title_changed, Ordering::Relaxed);
    }
    if stale > 0 {
        STATS[S_STALE].fetch_add(stale, Ordering::Relaxed);
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
    let stale = STATS[S_STALE].swap(0, Ordering::Relaxed) as f64 / f;
    let bytes = STATS[S_BYTES].swap(0, Ordering::Relaxed) as f64 / f;
    if notes > 0.0 || mism > 0.0 || stale > 0.0 {
        crate::xlog_info!(
            "D3D12.X: XCL_UPCOPY_CHECK per frame: {notes:.1} notes, {mism:.1} title-changed, {stale:.1} stale-held, {:.1} KB noted",
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
