//! Longer-lived host copies of title buffers (title↔card byte shadowing).
//!
//! Title GPU memory the host cannot read gets a persistent CPU-side view (`held`) on first use.
//! Land merges card readback against that view; copied-page generations track which 4 KiB pages
//! are current; a turn-robin sweep compares held bytes to title for diagnostics.
//!
//! GPL-2 reimplementation from understanding of CGBJ `victory-gpu` shadow @ `80e8619`
//! (study only — not a paste). Full card/D3D12/relay/window surface remains Wave C.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use super::land;

/// 4 KiB kernel page (copied-generation grain).
pub(crate) const PAGE: usize = 0x1000;
/// 64 KiB piece (land / differ grain; matches watch / sparse tile).
pub(crate) const PIECE: usize = 64 << 10;
const DIFF_CHUNK: usize = 32;

/// `XCL_SHADOW=0` forces off; otherwise on (absent counts as on). Also accepts `VICTORY_SHADOW`.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let off = std::env::var_os("XCL_SHADOW")
            .or_else(|| std::env::var_os("VICTORY_SHADOW"))
            .is_some_and(|v| v == "0");
        let on = !off;
        if on && (std::env::var_os("XCL_SHADOW").is_some() || std::env::var_os("VICTORY_SHADOW").is_some()) {
            crate::xlog_info!("D3D12.X: title↔card shadows on (XCL_SHADOW)");
        } else if off {
            crate::xlog_info!("D3D12.X: XCL_SHADOW=0 — title buffer shadows off");
        }
        on
    })
}

/// A title buffer's host copy (CPU-side card view + copied-page journal).
pub(crate) struct Shadow {
    pub(crate) start: u64,
    pub(crate) len: u64,
    pub(crate) id: u64,
    /// Bytes shaders / land last knew on the card side (same length as `len`).
    held: Mutex<Vec<u8>>,
    /// Per 4 KiB page from `start`: asking generation when last taken whole (0 = never).
    copied: Mutex<Vec<u64>>,
    /// Ranges shaders may still be writing: `(title_addr, end)`.
    pending: Mutex<Vec<(usize, usize)>>,
    /// Bitset of 4 KiB pages landed since last refresh look.
    landings: Mutex<Vec<u64>>,
    landing: AtomicBool,
}

struct Entry {
    end: u64,
    shadow: Option<Arc<Shadow>>,
}

static BUFFERS: RwLock<BTreeMap<(u64, usize), Entry>> = RwLock::new(BTreeMap::new());
/// Resource → start, so release can unregister without the title VA.
static BY_RES: Mutex<BTreeMap<usize, u64>> = Mutex::new(BTreeMap::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ASK_GEN: AtomicU64 = AtomicU64::new(1);
static ROBIN: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);

fn pages_for(start: u64, len: u64) -> usize {
    if len == 0 {
        return 0;
    }
    ((start + len).div_ceil(PAGE as u64) - start / PAGE as u64) as usize
}

/// Monotonic asking generation for [`Shadow::mark_copied`].
pub(crate) fn ask_gen() -> u64 {
    ASK_GEN.fetch_add(1, Ordering::Relaxed)
}

/// Directory entry: title buffer at `start` (`len` bytes), keyed also by `resource`.
pub(crate) fn register(resource: usize, start: u64, len: u64) {
    if !enabled() || len == 0 {
        return;
    }
    let mut map = BUFFERS.write().unwrap_or_else(|e| e.into_inner());
    map.insert(
        (start, resource),
        Entry {
            end: start.saturating_add(len),
            shadow: None,
        },
    );
    BY_RES.lock().unwrap_or_else(|e| e.into_inner()).insert(resource, start);
}

/// Title released the buffer; live `Arc`s keep the copy until lists drop them.
pub(crate) fn unregister(resource: usize, start: u64) {
    BUFFERS.write().unwrap_or_else(|e| e.into_inner()).remove(&(start, resource));
    let mut by = BY_RES.lock().unwrap_or_else(|e| e.into_inner());
    if by.get(&resource).copied() == Some(start) {
        by.remove(&resource);
    }
}

/// Unregister by resource id alone (sparse / COM release).
pub(crate) fn unregister_resource(resource: usize) {
    let start = BY_RES.lock().unwrap_or_else(|e| e.into_inner()).remove(&resource);
    if let Some(start) = start {
        BUFFERS.write().unwrap_or_else(|e| e.into_inner()).remove(&(start, resource));
    }
}

/// Innermost registered extent holding `address`, without making a copy.
pub(crate) fn extent(address: u64) -> Option<(u64, u64)> {
    let map = BUFFERS.read().unwrap_or_else(|e| e.into_inner());
    map.range(..=(address, usize::MAX))
        .rev()
        .take(64)
        .find(|(_, e)| e.end > address)
        .map(|(k, e)| (k.0, e.end - k.0))
}

/// Registered spans as `(start, len)`.
pub(crate) fn spans() -> Vec<(u64, u64)> {
    BUFFERS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(k, e)| (k.0, e.end - k.0))
        .collect()
}

/// Live shadow count (diagnostics / tests).
pub(crate) fn live_count() -> u64 {
    LIVE.load(Ordering::Relaxed)
}

/// Copy of the innermost title buffer holding `[address, address+len)`, made on first use.
pub(crate) fn find(address: u64, len: u64) -> Option<Arc<Shadow>> {
    if !enabled() {
        return None;
    }
    let end = address.checked_add(len.max(1))?;
    {
        let map = BUFFERS.read().unwrap_or_else(|e| e.into_inner());
        if let Some((_, Entry { shadow: Some(s), .. })) =
            map.range(..=(address, usize::MAX)).rev().take(64).find(|(_, e)| e.end >= end)
        {
            return Some(s.clone());
        }
    }
    let mut map = BUFFERS.write().unwrap_or_else(|e| e.into_inner());
    let key = map
        .range(..=(address, usize::MAX))
        .rev()
        .take(64)
        .find(|(_, e)| e.end >= end)
        .map(|(k, _)| *k)?;
    let entry = map.get_mut(&key)?;
    if entry.shadow.is_none() {
        let len = entry.end - key.0;
        entry.shadow = Some(Shadow::new(key.0, len));
    }
    entry.shadow.clone()
}

/// As [`find`], but only if the copy was already made.
pub(crate) fn peek(address: u64, len: u64) -> Option<Arc<Shadow>> {
    let end = address.checked_add(len.max(1))?;
    BUFFERS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .range(..=(address, usize::MAX))
        .rev()
        .take(64)
        .find(|(_, e)| e.end >= end)
        .and_then(|(_, e)| e.shadow.clone())
}

/// Ensure a registered buffer has a live copy (creates one if needed).
pub(crate) fn ensure(resource: usize, start: u64, len: u64) -> Option<Arc<Shadow>> {
    register(resource, start, len);
    find(start, 1)
}

impl Shadow {
    fn new(start: u64, len: u64) -> Arc<Self> {
        let pages = pages_for(start, len);
        LIVE.fetch_add(1, Ordering::Relaxed);
        Arc::new(Self {
            start,
            len,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            held: Mutex::new(vec![0u8; len as usize]),
            copied: Mutex::new(vec![0u64; pages]),
            pending: Mutex::default(),
            landings: Mutex::default(),
            landing: AtomicBool::new(false),
        })
    }

    /// Offsets in the copy for title `[address, address+len)`.
    pub(crate) fn inside(&self, address: usize, len: usize) -> (usize, usize) {
        let (start, end) = (self.start as usize, (self.start + self.len) as usize);
        let from = address.clamp(start, end);
        (from - start, (address + len).clamp(from, end) - start)
    }

    /// Mark whole 4 KiB pages fully inside title `[from, to)` as copied at `now`.
    pub(crate) fn mark_copied(&self, from: usize, to: usize, now: u64) {
        if from >= to || now == 0 {
            return;
        }
        let start = self.start as usize;
        let end = (self.start + self.len) as usize;
        let base = start / PAGE;
        let mut copied = self.copied.lock().unwrap_or_else(|e| e.into_inner());
        let last = (to - 1) / PAGE;
        for page in from / PAGE..=last {
            let lo = (page * PAGE).max(start);
            let hi = ((page + 1) * PAGE).min(end);
            if lo >= from && hi <= to && hi > lo {
                let i = page - base;
                if let Some(slot) = copied.get_mut(i) {
                    *slot = now;
                }
            }
        }
    }

    /// Copied generation for the page holding title address `at` (0 = never).
    pub(crate) fn copied_at(&self, at: usize) -> u64 {
        let start = self.start as usize;
        if at < start || at >= start + self.len as usize {
            return 0;
        }
        let i = at / PAGE - start / PAGE;
        self.copied.lock().unwrap_or_else(|e| e.into_inner()).get(i).copied().unwrap_or(0)
    }

    /// Declare shaders may write title `[address, address+len)`.
    pub(crate) fn declare_write(&self, address: usize, len: usize) {
        let end = address.saturating_add(len);
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !pending.iter().any(|&(a, e)| a == address && e == end) {
            pending.push((address, end));
        }
    }

    /// Drop declared write ranges after land-back.
    pub(crate) fn writes_done(&self, ranges: &[(usize, usize)]) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|r| !ranges.contains(r));
    }

    pub(crate) fn writes_pending(&self) -> bool {
        !self.pending.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// Note that landed GPU bytes touched title `[from, to)`.
    pub(crate) fn note_landing(&self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let first = self.start as usize / PAGE;
        let pages = (from / PAGE).saturating_sub(first)..to.div_ceil(PAGE).saturating_sub(first);
        let mut bits = self.landings.lock().unwrap_or_else(|e| e.into_inner());
        if bits.is_empty() {
            let all = pages_for(self.start, self.len);
            bits.resize(all.div_ceil(64), 0);
        }
        for page in pages {
            if let Some(word) = bits.get_mut(page / 64) {
                *word |= 1u64 << (page % 64);
            }
        }
        self.landing.store(true, Ordering::Release);
    }

    /// Take and clear landing bits overlapping title `[from, to)`; returns noted page runs
    /// as `(title_lo, title_hi, noted)`.
    pub(crate) fn take_landed(&self, from: usize, to: usize) -> Vec<(usize, usize, bool)> {
        let start = self.start as usize;
        let first = start / PAGE;
        let mut runs: Vec<(usize, usize, bool)> = Vec::new();
        {
            let mut bits = self.landings.lock().unwrap_or_else(|e| e.into_inner());
            let mut at = from.max(start);
            let end = to.min(start + self.len as usize);
            while at < end {
                let next = ((at / PAGE + 1) * PAGE).min(end);
                let page = at / PAGE - first;
                let noted = bits.get(page / 64).is_some_and(|w| (w >> (page % 64)) & 1 != 0);
                if noted && next - at == PAGE {
                    if let Some(word) = bits.get_mut(page / 64) {
                        *word &= !(1u64 << (page % 64));
                    }
                }
                match runs.last_mut() {
                    Some(last) if last.2 == noted && next - last.0 <= PIECE => last.1 = next,
                    _ => runs.push((at, next, noted)),
                }
                at = next;
            }
        }
        if bits_empty(&self.landings) {
            self.landing.store(false, Ordering::Release);
        }
        runs
    }

    /// Snapshot of the held card view.
    pub(crate) fn held_bytes(&self) -> Vec<u8> {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replace held bytes for offsets `[off, off+src.len)` inside the copy.
    pub(crate) fn put_held(&self, off: usize, src: &[u8]) {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if off >= held.len() {
            return;
        }
        let n = src.len().min(held.len() - off);
        held[off..off + n].copy_from_slice(&src[..n]);
    }

    /// Copy title bytes into held for whole pages marked stale vs `now` / `written`.
    /// `written` is `(page_index, last_write_gen)` in page order; pages with `write > copied` refresh.
    pub(crate) fn refresh_stale(&self, from: usize, to: usize, now: u64, written: &[(usize, u64)], title: &[u8]) -> usize {
        let start = self.start as usize;
        let (off0, off1) = self.inside(from, to.saturating_sub(from));
        if off0 >= off1 || title.len() < off1 - off0 {
            return 0;
        }
        let base = start / PAGE;
        let mut copied = self.copied.lock().unwrap_or_else(|e| e.into_inner());
        let mut writes = written.iter().peekable();
        let mut bytes = 0usize;
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let last = (to - 1) / PAGE;
        for page in from / PAGE..=last {
            let write = match writes.peek() {
                Some(&&(p, w)) if p == page => {
                    writes.next();
                    w
                }
                _ => 0,
            };
            let i = page.saturating_sub(base);
            let taken = copied.get(i).copied().unwrap_or(0);
            if taken != 0 && write <= taken {
                continue;
            }
            let lo = (page * PAGE).max(from).max(start);
            let hi = ((page + 1) * PAGE).min(to).min(start + self.len as usize);
            if lo >= hi {
                continue;
            }
            let a = lo - start;
            let b = hi - start;
            let t0 = lo - from;
            let t1 = hi - from;
            if t1 > title.len() || b > held.len() {
                continue;
            }
            if super::host::upcheck::on() {
                super::host::upcheck::note(self.id, a, &held[a..b], &title[t0..t1]);
            }
            held[a..b].copy_from_slice(&title[t0..t1]);
            bytes += b - a;
            if lo == (page * PAGE).max(start) && hi == ((page + 1) * PAGE).min(start + self.len as usize) {
                if let Some(slot) = copied.get_mut(i) {
                    *slot = now;
                }
            }
        }
        bytes
    }
}

fn bits_empty(bits: &Mutex<Vec<u64>>) -> bool {
    bits.lock().unwrap_or_else(|e| e.into_inner()).iter().all(|&w| w == 0)
}

/// Where `a` and `b` first differ within their common length.
pub(crate) fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    let len = a.len().min(b.len());
    let (a_chunks, a_tail) = a[..len].as_chunks::<DIFF_CHUNK>();
    let (b_chunks, b_tail) = b[..len].as_chunks::<DIFF_CHUNK>();
    let (offset, aa, bb) = match a_chunks.iter().zip(b_chunks).position(|(x, y)| x != y) {
        Some(i) => (i * DIFF_CHUNK, &a_chunks[i][..], &b_chunks[i][..]),
        None => (len - a_tail.len(), a_tail, b_tail),
    };
    aa.iter().zip(bb).position(|(x, y)| x != y).map(|i| offset + i)
}

/// Where `a` and `b` last differ within their common length.
pub(crate) fn last_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    let len = a.len().min(b.len());
    let (a_chunks, a_tail) = a[..len].as_chunks::<DIFF_CHUNK>();
    let (b_chunks, b_tail) = b[..len].as_chunks::<DIFF_CHUNK>();
    if let Some(i) = a_tail.iter().zip(b_tail).rposition(|(x, y)| x != y) {
        return Some(len - a_tail.len() + i);
    }
    let chunk = a_chunks.iter().zip(b_chunks).rposition(|(x, y)| x != y)?;
    a_chunks[chunk]
        .iter()
        .zip(&b_chunks[chunk])
        .rposition(|(x, y)| x != y)
        .map(|i| chunk * DIFF_CHUNK + i)
}

/// 64 KiB pieces of `[from, to)` (copy offsets) that differ from `before` (bytes from `from`).
/// `title_start` is the title VA of copy offset 0 (piece cuts follow title alignment).
pub(crate) fn differing_pieces(title_start: usize, from: usize, to: usize, host: &[u8], before: &[u8]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut at = from;
    while at < to {
        let n = (PIECE - (title_start + at) % PIECE).min(to - at);
        let h = &host[at - from..at - from + n];
        let b = &before[at - from..at - from + n];
        if h != b {
            found.push((at, n));
        }
        at += n;
    }
    found
}

/// After land merge: capture card bytes into any covering shadow's held view and note landings.
pub(crate) fn capture_after_land(dst: usize, card: &[u8], len: usize) {
    if !enabled() || len == 0 {
        return;
    }
    let len = len.min(card.len());
    let end = dst.saturating_add(len);
    let map = BUFFERS.read().unwrap_or_else(|e| e.into_inner());
    for (_, entry) in map.iter() {
        let Some(shadow) = entry.shadow.as_ref() else { continue };
        let (s0, s1) = (shadow.start as usize, (shadow.start + shadow.len) as usize);
        let lo = dst.max(s0);
        let hi = end.min(s1);
        if lo >= hi {
            continue;
        }
        let off = lo - s0;
        let src_off = lo - dst;
        shadow.put_held(off, &card[src_off..src_off + (hi - lo)]);
        shadow.note_landing(lo, hi);
        let now = ask_gen();
        shadow.mark_copied(lo, hi, now);
    }
}

/// Note landed ranges onto covering shadows (sparse beside path).
pub(crate) fn note_landed_ranges(ranges: &[(usize, usize)]) {
    if !enabled() {
        return;
    }
    for &(va, n) in ranges {
        if n == 0 {
            continue;
        }
        if let Some(s) = peek(va as u64, 1).or_else(|| find(va as u64, 1)) {
            s.note_landing(va, va + n);
        }
    }
}

/// Merge card bytes into title using a covering shadow's held view when one owns the span.
/// Returns `None` when no shadow covers the full range (caller uses its own held).
pub unsafe fn merge_if_covered(dst: usize, card: &[u8], len: usize) -> Option<(usize, usize)> {
    if !enabled() || len == 0 {
        return None;
    }
    let shadow = peek(dst as u64, len as u64)?;
    let (s0, s1) = (shadow.start as usize, (shadow.start + shadow.len) as usize);
    if dst < s0 || dst + len > s1 {
        return None;
    }
    let off = dst - s0;
    let mut held = shadow.held.lock().unwrap_or_else(|e| e.into_inner());
    if held.len() < off + len {
        held.resize(shadow.len as usize, 0);
    }
    let held_slice = &mut held[off..off + len];
    // SAFETY: caller guarantees `dst` is writable title memory of `len` bytes.
    let out = unsafe { land::merge_range(dst, card, held_slice, len) };
    drop(held);
    shadow.note_landing(dst, dst + len);
    Some(out)
}

/// Turn-robin: compare held pages believed current against `read_title(page_lo, into)`.
/// Stops after ~`most` bytes; advances the robin cursor. `say` gets (shadow_id, title_addr, dword_count, first).
pub(crate) fn stale_pages(
    most: usize,
    read_title: &mut dyn FnMut(usize, &mut [u8]) -> bool,
    say: &mut dyn FnMut(u64, usize, usize, (usize, u32, u32)),
) -> usize {
    if !enabled() || most == 0 {
        return 0;
    }
    let mut all: Vec<Arc<Shadow>> = BUFFERS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .filter_map(|e| e.shadow.clone())
        .collect();
    all.sort_by_key(|s| s.id);
    let next = ROBIN.load(Ordering::Relaxed);
    let turn = all.partition_point(|s| s.id < next);
    let mut done = 0usize;
    let mut title = vec![0u8; PAGE];
    for shadow in all[turn..].iter().chain(&all[..turn]) {
        if done >= most {
            ROBIN.store(shadow.id, Ordering::Relaxed);
            break;
        }
        let start = shadow.start as usize;
        let end = start + shadow.len as usize;
        let copied = shadow.copied.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let held = shadow.held.lock().unwrap_or_else(|e| e.into_inner());
        for (i, &at) in copied.iter().enumerate() {
            if at == 0 {
                continue;
            }
            let page = start / PAGE + i;
            let lo = (page * PAGE).max(start);
            let hi = ((page + 1) * PAGE).min(end);
            if lo >= hi {
                continue;
            }
            let buf = &mut title[..hi - lo];
            if !read_title(lo, buf) {
                continue;
            }
            done += hi - lo;
            let held_slice = &held[lo - start..hi - start];
            if held_slice == buf {
                continue;
            }
            let word = |b: &[u8], k: usize| u32::from_le_bytes([b[k], b[k + 1], b[k + 2], b[k + 3]]);
            let differ: Vec<usize> = (0..(hi - lo) & !3)
                .step_by(4)
                .filter(|&k| word(held_slice, k) != word(buf, k))
                .collect();
            if let Some(&k) = differ.first() {
                say(shadow.id, lo, differ.len(), (lo + k, word(held_slice, k), word(buf, k)));
            }
        }
    }
    done
}

/// Journal a 64 KiB land/watch block as landed on covering shadows (beside echo).
pub(crate) fn note_watch_block(block: usize) {
    if !enabled() {
        return;
    }
    let va = block * PIECE;
    note_landed_ranges(&[(va, PIECE)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_dir() {
        BUFFERS.write().unwrap().clear();
        BY_RES.lock().unwrap().clear();
        LIVE.store(0, Ordering::Relaxed);
        ROBIN.store(0, Ordering::Relaxed);
    }

    #[test]
    fn register_find_peek_unregister() {
        clear_dir();
        let _ = enabled(); // latch
        register(0xA1, 0x10000, 0x20000);
        assert_eq!(extent(0x18000), Some((0x10000, 0x20000)));
        assert!(peek(0x10000, 1).is_none());
        let s = find(0x18000, 0x100).expect("made");
        assert_eq!(s.start, 0x10000);
        assert_eq!(s.len, 0x20000);
        assert!(peek(0x18000, 1).is_some());
        assert_eq!(live_count(), 1);
        unregister(0xA1, 0x10000);
        assert!(peek(0x18000, 1).is_none());
    }

    #[test]
    fn unregister_by_resource() {
        clear_dir();
        register(7, 0x8000, 0x1000);
        let _ = find(0x8000, 1);
        unregister_resource(7);
        assert!(peek(0x8000, 1).is_none());
        assert!(spans().is_empty());
    }

    #[test]
    fn mark_copied_and_refresh_stale() {
        clear_dir();
        register(1, 0, 0x3000);
        let s = find(0, 1).unwrap();
        let now = ask_gen();
        s.mark_copied(0, 0x1000, now);
        assert_eq!(s.copied_at(0x100), now);
        assert_eq!(s.copied_at(0x1500), 0);

        let mut title = vec![0u8; 0x3000];
        title[0x1500] = 0xAB;
        // page 1 never copied → refresh; page 0 current with no newer write → skip
        let n = s.refresh_stale(0, 0x3000, ask_gen(), &[(1, 99)], &title);
        assert!(n >= 0x1000);
        assert_eq!(s.held_bytes()[0x1500], 0xAB);
    }

    #[test]
    fn differ_and_merge_basics() {
        clear_dir();
        let a = [0u8; 65];
        let mut b = [0u8; 65];
        b[32] = 1;
        b[64] = 2;
        assert_eq!(first_difference(&a, &b), Some(32));
        assert_eq!(last_difference(&a, &b), Some(64));
        assert_eq!(first_difference(&a, &a), None);

        let host = vec![0u8; PIECE + 8];
        let mut before = host.clone();
        before[PIECE + 1] = 9;
        let pieces = differing_pieces(0, 0, host.len(), &host, &before);
        assert!(!pieces.is_empty());
        assert!(pieces.iter().any(|&(at, _)| at >= PIECE));

        register(2, 0x10_0000, 64);
        let s = find(0x10_0000, 1).unwrap();
        let card = [0x11u8; 64];
        s.put_held(0, &[0u8; 64]);
        // Title is a local buffer; merge through land with shadow held.
        let mut title = [0u8; 64];
        let mut held = s.held_bytes();
        let (nb, dw) = unsafe { land::merge_range(title.as_mut_ptr() as usize, &card, &mut held, 64) };
        assert!(nb > 0 || dw > 0 || title == card);
        s.put_held(0, &held);
        capture_after_land(0x10_0000, &card, 64);
        // Local title address will not match registered VA; exercise note + take on the shadow directly.
        s.note_landing(0x10_0000, 0x10_0000 + 64);
        let runs = s.take_landed(0x10_0000, 0x10_0000 + 64);
        assert!(!runs.is_empty());
        assert!(runs.iter().any(|r| r.2));
    }

    #[test]
    fn turn_robin_reports_stale() {
        clear_dir();
        register(3, 0, PAGE as u64 * 2);
        let s = find(0, 1).unwrap();
        let now = ask_gen();
        s.mark_copied(0, PAGE * 2, now);
        s.put_held(0, &vec![0u8; PAGE * 2]);
        let mut title_mem = vec![0u8; PAGE * 2];
        title_mem[4] = 0xFF;
        let mut reports = 0usize;
        let n = stale_pages(
            PAGE * 4,
            &mut |lo, into| {
                into.copy_from_slice(&title_mem[lo..lo + into.len()]);
                true
            },
            &mut |_, _, _, _| {
                reports += 1;
            },
        );
        assert!(n > 0);
        assert!(reports >= 1);
    }

    #[test]
    fn differences_match_byte_scan() {
        let base: Vec<u8> = (0..98u32).map(|i| (i * 7 + 3) as u8).collect();
        for len in [0usize, 1, 31, 32, 33, 63, 64, 65, 97] {
            let a = &base[1..1 + len];
            assert_eq!(first_difference(a, a), None);
            for at in 0..len {
                let mut b = a.to_vec();
                b[at] ^= 0x80;
                assert_eq!(first_difference(a, &b), Some(at));
                assert_eq!(last_difference(a, &b), Some(at));
            }
        }
    }
}
