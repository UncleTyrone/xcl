//! Torn-fetch detection (`XCL_FETCH_CHECK=1` with relay copy).
//!
//! Pieces fetched while a later flight may still write them are hashed; a
//! mismatch marks the piece suspect so the next flight that may write it
//! fetches again regardless of the compare flag.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::super::diag_env;

pub(crate) fn on() -> bool {
    diag_env::fetch_check()
}

/// `(copy id, piece index)`.
pub(crate) type Piece = (u64, u32);

static STATS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static SUSPECTS: OnceLock<Mutex<HashSet<Piece>>> = OnceLock::new();

fn suspects() -> &'static Mutex<HashSet<Piece>> {
    SUSPECTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether a flight that may write `piece` should fetch it.
pub(crate) fn wanted(piece: Piece, flagged: bool) -> bool {
    if !on() {
        return flagged;
    }
    if flagged {
        return true;
    }
    suspects()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&piece)
}

/// `piece` was fetched; `same` means it matched the flight-end hash.
pub(crate) fn fetched(piece: Piece, same: bool) {
    if !on() {
        return;
    }
    STATS[0].fetch_add(1, Ordering::Relaxed);
    let mut s = suspects().lock().unwrap_or_else(|e| e.into_inner());
    if same {
        if s.remove(&piece) {
            STATS[2].fetch_add(1, Ordering::Relaxed);
        }
    } else {
        s.insert(piece);
        STATS[1].fetch_add(1, Ordering::Relaxed);
    }
}

/// FNV-1a over up to 64 KiB (fast torn check companion to card compare hashes).
pub(crate) fn hash_bytes(bytes: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for &b in bytes.iter().take(64 << 10) {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

pub(crate) fn line(frames: f64) {
    if !on() {
        return;
    }
    let f = frames.max(1.0);
    let hashed = STATS[0].swap(0, Ordering::Relaxed) as f64 / f;
    let suspect = STATS[1].swap(0, Ordering::Relaxed) as f64 / f;
    let cleared = STATS[2].swap(0, Ordering::Relaxed) as f64 / f;
    let open = suspects().lock().unwrap_or_else(|e| e.into_inner()).len();
    if hashed > 0.0 || open > 0 {
        crate::xlog_info!(
            "D3D12.X: XCL_FETCH_CHECK per frame: {hashed:.1} hashed, {suspect:.1} suspect, {cleared:.1} cleared; {open} open"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_differs_on_change() {
        assert_ne!(hash_bytes(&[1, 2, 3]), hash_bytes(&[1, 2, 4]));
    }

    #[test]
    fn suspect_forces_fetch() {
        if !on() {
            assert!(wanted((1, 0), true));
            assert!(!wanted((1, 0), false));
        }
    }
}
