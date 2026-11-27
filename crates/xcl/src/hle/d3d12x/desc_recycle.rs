//! Fence-quarantined recycle for shader-visible descriptor heap ranges.
//!
//! Released PC indices stay off the free list until every live title queue on
//! the device has passed a fence signal recorded at release time (CGBJ
//! `list::release_tables` / long-session dynamic slot pressure).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;

/// Add a released range to `free`, merging adjacent runs.
pub(crate) fn merge_release(free: &mut Vec<(u32, u32)>, first: u32, count: u32) {
    free.push((first, count));
    free.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(free.len());
    for (f, n) in free.drain(..) {
        match out.last_mut() {
            Some(last) if f <= last.0 + last.1 => last.1 = last.1.max(f + n - last.0),
            _ => out.push((f, n)),
        }
    }
    *free = out;
}

fn ranges_overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.0.saturating_add(b.1) && b.0 < a.0.saturating_add(a.1)
}

/// Active destination copies and deferred returns while a range is still leased.
#[derive(Default)]
pub(crate) struct LeasePool {
    leased: Vec<(u32, u32)>,
    deferred: Vec<(u32, u32)>,
}

impl LeasePool {
    pub(crate) fn acquire(&mut self, first: u32, count: u32) {
        if count > 0 {
            self.leased.push((first, count));
        }
    }

    pub(crate) fn release_or_defer(&mut self, free: &mut Vec<(u32, u32)>, first: u32, count: u32) -> bool {
        if self.leased.iter().any(|&l| ranges_overlap(l, (first, count))) {
            self.deferred.push((first, count));
            false
        } else {
            merge_release(free, first, count);
            true
        }
    }

    pub(crate) fn end_leases(&mut self, free: &mut Vec<(u32, u32)>, ranges: &[(u32, u32)]) -> bool {
        for &r in ranges {
            if let Some(i) = self.leased.iter().position(|&x| x == r) {
                self.leased.swap_remove(i);
            }
        }
        let mut promoted = false;
        let mut i = 0;
        while i < self.deferred.len() {
            let r = self.deferred[i];
            if !self.leased.iter().any(|&l| ranges_overlap(l, r)) {
                self.deferred.swap_remove(i);
                merge_release(free, r.0, r.1);
                promoted = true;
            } else {
                i += 1;
            }
        }
        promoted
    }
}

/// Released range awaiting GPU queue fence completion before re-entering `free`.
pub(crate) struct RetiredRange<F> {
    pub(crate) first: u32,
    pub(crate) count: u32,
    pub(crate) marks: Option<Vec<(F, u64)>>,
}

pub(crate) fn drain_retired_ranges<F>(
    retired: &mut VecDeque<RetiredRange<F>>,
    free: &mut Vec<(u32, u32)>,
    leases: &mut LeasePool,
    mut completed: impl FnMut(&F) -> u64,
) -> bool {
    let mut drained = false;
    while retired
        .front()
        .is_some_and(|r| r.marks.as_ref().is_some_and(|m| m.iter().all(|(f, v)| completed(f) >= *v)))
    {
        if let Some(r) = retired.pop_front() {
            if leases.release_or_defer(free, r.first, r.count) {
                drained = true;
            }
        }
    }
    drained
}

struct QueueFences(HashMap<usize, (ID3D12Fence, u64)>);

unsafe impl Send for QueueFences {}

static HEAP_FENCES: Mutex<Option<QueueFences>> = Mutex::new(None);

/// Signal every known live title queue on `dev`. `Some(vec![])` when none are live;
/// `None` when fence signal fails (caller must quarantine without draining).
pub(crate) unsafe fn signal_live_queues_for(dev: &ID3D12Device) -> Option<Vec<(ID3D12Fence, u64)>> {
    let dev_raw = dev.as_raw();
    let mut queues: Vec<(usize, ID3D12CommandQueue)> = Vec::new();
    for raw in super::present::known_queues() {
        let ptr = raw as *mut std::ffi::c_void;
        if let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&ptr) {
            let mut qdev: Option<ID3D12Device> = None;
            if q.GetDevice(&mut qdev).is_ok() && qdev.as_ref().is_some_and(|d| d.as_raw() == dev_raw) {
                queues.push((raw, q.clone()));
            }
        }
    }
    if queues.is_empty() {
        return Some(Vec::new());
    }
    let mut guard = HEAP_FENCES.lock().unwrap();
    let map = &mut guard.get_or_insert_with(|| QueueFences(HashMap::new())).0;
    let mut marks = Vec::with_capacity(queues.len());
    for (key, q) in queues {
        let (fence, val) = match map.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let f: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
                e.insert((f, 0))
            }
        };
        let next = *val + 1;
        if q.Signal(&*fence, next).is_err() {
            return None;
        }
        *val = next;
        marks.push((fence.clone(), next));
    }
    Some(marks)
}
