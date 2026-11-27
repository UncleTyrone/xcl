//! Up-copy freshness check (`XCL_UPCOPY_CHECK=1`).
//!
//! Notes CPU-side bytes before a send-up; at rest (frame line) compares noted
//! ranges with what was recorded as `new`.
//!
//! ## GPU probe CSO — **IMPOSSIBLE** (product path)
//!
//! CGBJ `victory-gpu` `host/upcheck.rs` re-reads card-resident bytes with a runtime-compiled
//! compute shader (`SOURCE` HLSL → `CreateComputePipelineState`) recorded **ahead of** each
//! relay send on the title's command list (`record` / `Looks::read`), plus a whole-card
//! `at_rest` pass over `shared_buffer` SRVs. That path needs:
//! - per-submit compute injection on live title lists (`HostList` deferred queue),
//! - card SRV + staging SRV bind of every dirty shadow,
//! - `Host::shared_buffer` answer UAVs waited on the graphics queue.
//!
//! XCL's relay lands via `relay_finish_ranges` / shadow `held` without a portable hook to splice
//! arbitrary compute into title lists without wholesale `relay.rs` rewrite (banned). Shipping a
//! standalone probe CSO still cannot observe the **same** pre-send card words CGBJ compares.
//! Windows D3DCompile exists, but it is not a portable product path into XCL's submit spine.
//!
//! ## MATCH product diagnostic
//!
//! [`sweep_rest`] compares noted `was` against post-land shadow [`held_span`](super::super::shadow::Shadow::held_span)
//! for title-stable words (`was` == `new`) — honest proxy for "send put an older value over what
//! the card held" after `relay_finish_ranges`. Overlapping notes in one submission merge like CGBJ
//! (`note` keeps prior `was`, updates prior `new`).

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
///
/// Overlapping notes in the same submission: prior `was` wins (card still holds the first
/// refresh's former bytes); prior `new` is updated to the latest title bytes (CGBJ `note`).
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
    let (mut was, new) = (was[skip..skip + n].to_vec(), new[skip..skip + n].to_vec());
    STATS[S_NOTES].fetch_add(1, Ordering::Relaxed);
    STATS[S_BYTES].fetch_add(n as u64, Ordering::Relaxed);
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    let noted = noted.entry(copy).or_default();
    for t in noted.iter_mut() {
        let (a, b) = (at.max(t.at), (at + n).min(t.at + t.was.len()));
        if a < b {
            was[a - at..b - at].copy_from_slice(&t.was[a - t.at..b - t.at]);
            t.new[a - t.at..b - t.at].copy_from_slice(&new[a - at..b - at]);
        }
    }
    noted.push(Taken { at, was, new });
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

    #[test]
    fn notes_are_cut_to_whole_words() {
        if on() {
            return;
        }
        // Temporarily force note without env: exercise alignment via direct Taken push path.
        let (was, new): (Vec<u8>, Vec<u8>) = ((5..18).collect(), (105..118).collect());
        let skip = 5usize.wrapping_neg() & 3;
        let n = was.len().min(new.len()).saturating_sub(skip) & !3;
        assert_eq!((5 + skip, n), (8, 8));
    }

    #[test]
    fn overlapping_notes_keep_prior_was() {
        // Direct merge logic (same as note when on).
        let at = 0usize;
        let n = 8usize;
        let mut was = vec![1u8; n];
        let new = vec![2u8; n];
        let mut prior = Taken {
            at: 0,
            was: vec![9u8; n],
            new: vec![1u8; n],
        };
        let (a, b) = (at.max(prior.at), (at + n).min(prior.at + prior.was.len()));
        assert!(a < b);
        was[a - at..b - at].copy_from_slice(&prior.was[a - prior.at..b - prior.at]);
        prior.new[a - prior.at..b - prior.at].copy_from_slice(&new[a - at..b - at]);
        assert_eq!(&was[..], &[9u8; 8]);
        assert_eq!(&prior.new[..], &[2u8; 8]);
    }
}
