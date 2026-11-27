//! Card staging metadata + optional D3D12 twin buffers for large shadows (GPL-2 rewrite).
//!
//! Tracks 64 KiB dirty runs on the host-held card view and send generations for relay
//! flights. When a live D3D12 device is available, also allocates an UPLOAD staging buffer
//! (CPU side) and a DEFAULT UAV "before" twin (card side) matching CGBJ `Card::{staging,before}`
//! roles — study only, not a paste. On-card hash/compare CSOs remain with `card_steps` / land.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

use super::real;
use super::shadow::PIECE;

static TWIN_BYTES: AtomicU64 = AtomicU64::new(0);
static TWIN_MADE: AtomicU64 = AtomicU64::new(0);
static BEFORE_MADE: AtomicU64 = AtomicU64::new(0);

static SEND_SEQ: AtomicU64 = AtomicU64::new(1);

/// Ranges sent up to the card side, keyed by send order (relay).
#[derive(Default)]
struct SentRuns(Vec<(u64, Vec<(usize, usize)>)>);

impl SentRuns {
    fn note_joined(&mut self, number: u64, ranges: Vec<(usize, usize)>) {
        if ranges.is_empty() {
            return;
        }
        match self.0.iter_mut().find(|s| s.0 == number) {
            Some((_, all)) => *all = joined([std::mem::take(all), ranges].concat()),
            None => self.0.push((number, ranges)),
        }
    }

    fn forget(&mut self, number: u64) {
        self.0.retain(|s| s.0 > number);
    }

    /// Appends sent ranges after `number` that overlap `[from, to)` into `out`.
    fn near(&self, number: u64, from: usize, to: usize, out: &mut Vec<(usize, usize)>) {
        for (_, ranges) in self.0.iter().filter(|s| s.0 > number) {
            let first = ranges.partition_point(|r| r.1 <= from);
            out.extend(
                ranges[first..]
                    .iter()
                    .take_while(|r| r.0 < to)
                    .copied(),
            );
        }
    }
}

/// Ranges in order; overlapping or touching spans collapse to one.
pub(crate) fn joined(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (from, to) in ranges {
        match out.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => out.push((from, to)),
        }
    }
    out
}

/// `[from, to)` without `skip` intervals, in order. `skip` entries are `(lo, hi)`.
pub(crate) fn outside(from: usize, to: usize, mut skip: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    skip.sort_unstable();
    let mut out = Vec::new();
    let mut at = from;
    for (a, b) in skip {
        if b <= at {
            continue;
        }
        if a >= to {
            break;
        }
        if a > at {
            out.push((at, a));
        }
        at = at.max(b);
    }
    if at < to {
        out.push((at, to));
    }
    out
}

/// Merge GPU/`now` bytes into `held` and `title` where they differ from the prior held view.
/// Returns whether any title byte changed. Does not write into `now`'s source.
pub(crate) fn merge_piece(now: &[u8], held: &mut [u8], title: &mut [u8]) -> bool {
    let n = now.len().min(held.len()).min(title.len());
    let mut differs = 0u8;
    for i in 0..n {
        let wrote = if now[i] != held[i] { 0xffu8 } else { 0 };
        let merged = (title[i] & !wrote) | (now[i] & wrote);
        differs |= merged ^ title[i];
        title[i] = merged;
        held[i] = (held[i] & !wrote) | (now[i] & wrote);
    }
    differs != 0
}

/// D3D12 host staging + optional card-side "before" twin (CGBJ `Card` role).
pub(crate) struct CardTwin {
    pub(crate) staging: ID3D12Resource,
    pub(crate) staging_gpu: u64,
    pub(crate) size: u64,
    before: Mutex<Option<ID3D12Resource>>,
}

impl CardTwin {
    pub(crate) fn before_clone(&self) -> Option<ID3D12Resource> {
        self.before.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Per-shadow card side when [`session::card_buffers_room`] admits the buffer.
pub(crate) struct CardSide {
    pub(crate) size: u64,
    dirty: Mutex<Vec<(usize, usize)>>,
    sent: Mutex<SentRuns>,
    touched: AtomicBool,
    /// Optional D3D12 staging (+ lazy before twin). Absent when no device / Create refused.
    twin: Option<CardTwin>,
}

fn buf_desc(size: u64, flags: D3D12_RESOURCE_FLAGS) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: size.max(1).next_multiple_of(PIECE as u64),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
    }
}

/// UPLOAD heap staging buffer for the CPU's card side.
pub(crate) fn try_alloc_staging(size: u64) -> Option<(ID3D12Resource, u64)> {
    let raw = real::device_raw();
    if raw.is_null() || size == 0 {
        return None;
    }
    let device = unsafe { ID3D12Device::from_raw_borrowed(&raw)? };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut buf: Option<ID3D12Resource> = None;
    unsafe {
        device
            .CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &buf_desc(size, D3D12_RESOURCE_FLAG_NONE),
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut buf,
            )
            .ok()?;
    }
    let buf = buf?;
    let gpu = unsafe { buf.GetGPUVirtualAddress() };
    Some((buf, gpu))
}

/// DEFAULT UAV buffer for the card-side "before" twin.
pub(crate) fn try_alloc_before(size: u64) -> Option<ID3D12Resource> {
    let raw = real::device_raw();
    if raw.is_null() || size == 0 {
        return None;
    }
    let device = unsafe { ID3D12Device::from_raw_borrowed(&raw)? };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut buf: Option<ID3D12Resource> = None;
    unsafe {
        device
            .CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &buf_desc(size, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS),
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                None,
                &mut buf,
            )
            .ok()?;
    }
    buf
}

impl CardSide {
    pub(crate) fn new(len: u64) -> Self {
        let size = len.max(1).next_multiple_of(PIECE as u64);
        let twin = try_alloc_staging(size).map(|(staging, staging_gpu)| {
            TWIN_MADE.fetch_add(1, Ordering::Relaxed);
            TWIN_BYTES.fetch_add(size, Ordering::Relaxed);
            let n = TWIN_MADE.load(Ordering::Relaxed);
            if n <= 8 || n.is_power_of_two() {
                crate::xlog_info!(
                    "D3D12.X: card staging twin {} KiB ({} live twin bytes)",
                    size >> 10,
                    TWIN_BYTES.load(Ordering::Relaxed)
                );
            }
            CardTwin {
                staging,
                staging_gpu,
                size,
                before: Mutex::default(),
            }
        });
        Self {
            size: len,
            dirty: Mutex::default(),
            sent: Mutex::default(),
            touched: AtomicBool::new(false),
            twin,
        }
    }

    /// CPU staging + optional before twin + committed size (CGBJ `card_sides`).
    pub(crate) fn card_sides(&self) -> Option<(&ID3D12Resource, Option<ID3D12Resource>, u64)> {
        let twin = self.twin.as_ref()?;
        Some((&twin.staging, twin.before_clone(), twin.size))
    }

    pub(crate) fn has_twin(&self) -> bool {
        self.twin.is_some()
    }

    pub(crate) fn staging_gpu(&self) -> Option<u64> {
        self.twin.as_ref().map(|t| t.staging_gpu)
    }

    /// Lazy-make the DEFAULT "before" twin with `make` (or [`try_alloc_before`]).
    /// Returns whether one was created now.
    pub(crate) fn card_make_before(
        &self,
        make: impl FnOnce(u64) -> Option<ID3D12Resource>,
    ) -> bool {
        let Some(twin) = &self.twin else {
            return false;
        };
        let mut before = twin.before.lock().unwrap_or_else(|e| e.into_inner());
        if before.is_some() {
            return false;
        }
        *before = make(twin.size);
        if before.is_some() {
            BEFORE_MADE.fetch_add(1, Ordering::Relaxed);
            TWIN_BYTES.fetch_add(twin.size, Ordering::Relaxed);
        }
        before.is_some()
    }

    /// Ensure a before twin exists (allocates DEFAULT UAV when missing).
    pub(crate) fn ensure_before(&self) -> bool {
        self.card_make_before(try_alloc_before)
    }

    pub(crate) fn note_dirty(&self, off: usize, len: usize) {
        if len == 0 {
            return;
        }
        let end = off.saturating_add(len);
        let mut d = self.dirty.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = d.last_mut() {
            if last.1 == off {
                last.1 = end;
                self.touched.store(true, Ordering::Release);
                return;
            }
        }
        d.push((off, end));
        self.touched.store(true, Ordering::Release);
    }

    /// Take dirty runs (joined), optionally journaling them as sent under `sent_as`.
    pub(crate) fn take_dirty(&self, sent_as: Option<u64>, fetched_through: u64) -> Vec<(usize, usize)> {
        if !self.touched.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }
        let mut d = self.dirty.lock().unwrap_or_else(|e| e.into_inner());
        let ranges = joined(std::mem::take(&mut *d));
        if let Some(order) = sent_as {
            let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
            sent.forget(fetched_through);
            sent.note_joined(order, ranges.clone());
        }
        ranges
    }

    pub(crate) fn note_sent(&self, runs: &[(usize, usize)], order: u64) {
        if runs.is_empty() {
            return;
        }
        let joined_runs = joined(runs.iter().map(|&(a, n)| (a, a + n)).collect());
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .note_joined(order, joined_runs);
    }

    pub(crate) fn sent_older_than(&self, order: u64) -> Vec<(usize, usize)> {
        let sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for (o, ranges) in &sent.0 {
            if *o < order {
                out.extend(ranges.iter().map(|&(a, b)| (a, b.saturating_sub(a))));
            }
        }
        out
    }

    pub(crate) fn clear_sent_through(&self, order: u64) {
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget(order);
    }

    /// Skip ranges for relay card merge: dirty ∪ sent after `fetched_after`, clipped to `[at, at+n)`.
    pub(crate) fn skip_for_relay(&self, fetched_after: u64, at: usize, n: usize) -> Vec<(usize, usize)> {
        let end = at.saturating_add(n);
        let dirty = self.dirty.lock().unwrap_or_else(|e| e.into_inner());
        let sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        let mut skip: Vec<(usize, usize)> = dirty
            .iter()
            .copied()
            .filter(|&(a, b)| a < end && b > at)
            .collect();
        sent.near(fetched_after, at, end, &mut skip);
        skip
    }

    pub(crate) fn touched(&self) -> bool {
        self.touched.load(Ordering::Acquire)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joined_and_outside() {
        assert_eq!(joined(vec![(0, 10), (10, 20), (25, 30)]), vec![(0, 20), (25, 30)]);
        assert_eq!(
            outside(0, 100, vec![(10, 20), (50, 60)]),
            vec![(0, 10), (20, 50), (60, 100)]
        );
    }

    #[test]
    fn sent_near_and_forget() {
        let mut s = SentRuns::default();
        s.note_joined(1, vec![(0, 64), (128, 192)]);
        s.note_joined(3, vec![(32, 96)]);
        let mut out = Vec::new();
        s.near(1, 0, 200, &mut out);
        assert_eq!(out, vec![(32, 96)]);
        s.forget(1);
        assert!(s.0.iter().all(|(o, _)| *o > 1));
    }

    #[test]
    fn merge_piece_takes_gpu_where_held_moved() {
        let now = [1u8, 2, 3, 4];
        let mut held = [0u8, 0, 3, 4];
        let mut title = [9u8, 9, 3, 4];
        assert!(merge_piece(&now, &mut held, &mut title));
        assert_eq!(title, [1, 2, 3, 4]);
        assert_eq!(held, [1, 2, 3, 4]);
    }
}
