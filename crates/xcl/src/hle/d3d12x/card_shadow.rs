//! CPU-side card staging metadata for large buffer shadows (GPL-2; not full D3D12 twin buffers).
//!
//! Tracks 64 KiB dirty runs on the host-held card view and send generations for relay flights.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use super::shadow::PIECE;
use super::session;

static SEND_SEQ: AtomicU64 = AtomicU64::new(1);

/// Per-shadow card side when [`session::card_buffers_room`] admits the buffer.
pub(crate) struct CardSide {
    pub(crate) size: u64,
    dirty: Mutex<Vec<(usize, usize)>>,
    sent: Mutex<Vec<(usize, usize, u64)>>,
    touched: AtomicBool,
}

impl CardSide {
    pub(crate) fn new(len: u64) -> Self {
        Self {
            size: len,
            dirty: Mutex::default(),
            sent: Mutex::default(),
            touched: AtomicBool::new(false),
        }
    }

    pub(crate) fn note_dirty(&self, off: usize, len: usize) {
        if len == 0 {
            return;
        }
        let mut d = self.dirty.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = d.last_mut() {
            if last.0 + last.1 == off {
                last.1 += len;
                self.touched.store(true, Ordering::Release);
                return;
            }
        }
        d.push((off, len));
        self.touched.store(true, Ordering::Release);
    }

    pub(crate) fn take_dirty(&self) -> Vec<(usize, usize)> {
        let mut d = self.dirty.lock().unwrap_or_else(|e| e.into_inner());
        if d.is_empty() {
            return Vec::new();
        }
        let out = std::mem::take(&mut *d);
        self.touched.store(false, Ordering::Release);
        out
    }

    pub(crate) fn note_sent(&self, runs: &[(usize, usize)], order: u64) {
        if runs.is_empty() {
            return;
        }
        let mut s = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        s.extend(runs.iter().map(|&(a, n)| (a, n, order)));
    }

    pub(crate) fn sent_older_than(&self, order: u64) -> Vec<(usize, usize)> {
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, _, o)| *o < order)
            .map(|&(a, n, _)| (a, n))
            .collect()
    }

    pub(crate) fn clear_sent_through(&self, order: u64) {
        let mut s = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        s.retain(|(_, _, o)| *o >= order);
    }
}

pub(crate) fn next_send_order() -> u64 {
    SEND_SEQ.fetch_add(1, Ordering::AcqRel)
}

/// Merge dirty piece runs into held/title when under declared shader writes.
pub(crate) fn merge_piece_runs(
    title_start: usize,
    lo: usize,
    n: usize,
    held: &mut [u8],
    title: &[u8],
) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut at = 0usize;
    while at < n {
        let piece = (PIECE - (title_start + lo + at) % PIECE).min(n - at);
        if title[at..at + piece] != held[at..at + piece] {
            runs.push((at, piece));
            held[at..at + piece].copy_from_slice(&title[at..at + piece]);
        }
        at += piece;
    }
    runs
}