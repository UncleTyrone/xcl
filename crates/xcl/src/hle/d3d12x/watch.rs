//! Belobug renderer ports 6 and 7 for amd-dev: shared refresh/fault coordination and a sharded
//! dirty-page journal (reimplemented for XCL; not a copy of victory-gpu).
//!
//! Port 6 (`XCL_PORT_GUARD`): faults and write-watch refreshes share one lock; each 64 KiB block
//! is claimed while its protection or upload state changes.
//! Port 7 (`XCL_PORT_JOURNAL`): journal notes are sharded (one lock per shard); notes that only
//! repeat the standing generation take the outer journal lock shared.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard};

use super::port;

/// Console / sparse tile size (64 KiB).
pub(crate) const BLOCK: usize = 0x1_0000;
const KERNEL_PAGE: usize = 0x1000;
const PARTS: usize = BLOCK / KERNEL_PAGE;
const WHOLE: u16 = u16::MAX;

const SHARD_SHIFT: u32 = 10;
const SHARDS: usize = 64;

fn shard_of(page: usize) -> usize {
    (page >> SHARD_SHIFT) % SHARDS
}

fn shards_of_pages<'a, I: IntoIterator<Item = usize>>(pages: I) -> u64 {
    pages.into_iter().fold(0u64, |m, page| m | (1u64 << shard_of(page)))
}

fn block_of_addr(addr: usize) -> usize {
    addr / BLOCK
}

// ---------------------------------------------------------------------------
// Port 6: shared guard lock and per-64 KiB claims
// ---------------------------------------------------------------------------

static GUARD_LOCK: RwLock<()> = RwLock::new(());

struct BlockBusy {
    words: Mutex<Vec<AtomicU64>>,
}

impl BlockBusy {
    const fn new() -> Self {
        Self { words: Mutex::new(Vec::new()) }
    }

    fn try_set(&self, block: usize) -> bool {
        let word = block / 64;
        let bit = 1u64 << (block % 64);
        let mut g = self.words.lock().unwrap_or_else(|e| e.into_inner());
        while g.len() <= word {
            g.push(AtomicU64::new(0));
        }
        let w = &g[word];
        loop {
            let cur = w.load(Ordering::Acquire);
            if cur & bit != 0 {
                return false;
            }
            if w.compare_exchange(cur, cur | bit, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return true;
            }
        }
    }

    fn clear(&self, block: usize) {
        let word = block / 64;
        let bit = 1u64 << (block % 64);
        let g = self.words.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = g.get(word) {
            w.fetch_and(!bit, Ordering::Release);
        }
    }
}

static BUSY: BlockBusy = BlockBusy::new();

static PORT6: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
const P6_FAULTS: usize = 0;
const P6_FAULT_WAIT_NS: usize = 1;
const P6_REFRESH_SHARED: usize = 2;
const P6_CLAIMS_MISSED: usize = 3;
const P6_CLAIM_SPINS: usize = 4;

fn port6_count(what: usize, n: u64) {
    PORT6[what].fetch_add(n, Ordering::Relaxed);
}

/// Blocks held until dropped ([`claim_blocks`]).
pub(crate) struct BlockClaims {
    blocks: Vec<usize>,
}

/// Wait for and take claims on `blocks` (sorted ascending, each once).
pub(crate) fn claim_blocks(blocks: &[usize]) -> BlockClaims {
    let mut blocks: Vec<usize> = blocks.to_vec();
    blocks.sort_unstable();
    blocks.dedup();
    let mut spun = 0u64;
    for &block in &blocks {
        let mut tries = 0u32;
        while !BUSY.try_set(block) {
            tries += 1;
            spun += 1;
            if tries < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
    }
    if spun != 0 {
        port6_count(P6_CLAIM_SPINS, spun);
    }
    BlockClaims { blocks }
}

impl Drop for BlockClaims {
    fn drop(&mut self) {
        for &block in &self.blocks {
            BUSY.clear(block);
        }
    }
}

/// Serialize refresh when port 6 is off (legacy behaviour).
static REFRESH_SERIAL: Mutex<()> = Mutex::new(());

/// Held for the duration of a title `refresh` / write-watch scan.
pub(crate) struct RefreshGate {
    _shared: Option<RwLockReadGuard<'static, ()>>,
    _serial: Option<MutexGuard<'static, ()>>,
}

impl RefreshGate {
    fn enter() -> Self {
        if port::guard() {
            port6_count(P6_REFRESH_SHARED, 1);
            RefreshGate { _shared: Some(GUARD_LOCK.read().unwrap_or_else(|e| e.into_inner())), _serial: None }
        } else {
            RefreshGate { _shared: None, _serial: Some(REFRESH_SERIAL.lock().unwrap_or_else(|e| e.into_inner())) }
        }
    }
}

/// Lock taken around a write-watch refresh (`title_textures::refresh_once`).
pub(crate) fn refresh_gate() -> RefreshGate {
    RefreshGate::enter()
}

/// Run `f` under the shared guard lock when port 6 is on, else with no extra lock.
pub(crate) fn with_guards_shared<R>(f: impl FnOnce() -> R) -> R {
    if port::guard() {
        let _g = GUARD_LOCK.read().unwrap_or_else(|e| e.into_inner());
        f()
    } else {
        f()
    }
}

/// A view-memory write fault: open the 64 KiB block under shared lock + claim (port 6), else
/// return `None` so the caller uses the legacy path.
pub(crate) fn view_fault_begin(view: usize) -> Option<(BlockClaims, std::time::Duration)> {
    if !port::guard() {
        return None;
    }
    let block = block_of_addr(view);
    let began = std::time::Instant::now();
    let _read = GUARD_LOCK.read().unwrap_or_else(|e| e.into_inner());
    let waited = began.elapsed();
    port6_count(P6_FAULT_WAIT_NS, waited.as_nanos() as u64);
    let claims = claim_blocks(&[block]);
    port6_count(P6_FAULTS, 1);
    Some((claims, waited))
}

/// Unique 64 KiB tile bases for addresses in `dirty` (`page` bytes per step).
pub(crate) fn blocks_for_dirty(dirty: &[usize], page: usize) -> Vec<usize> {
    let step = page.max(KERNEL_PAGE);
    let mut blocks: Vec<usize> = dirty.iter().map(|&p| block_of_addr(p)).collect();
    blocks.sort_unstable();
    blocks.dedup();
    let _ = step;
    blocks
}

/// Whether submit-side refresh planning runs beside the lock on worker threads (port 6 plan).
pub(crate) fn guard_plan_on() -> bool {
    port::guard_plan()
}

pub(crate) fn port6_line(frames: f64) {
    if !port::guard() && PORT6.iter().all(|c| c.load(Ordering::Relaxed) == 0) {
        return;
    }
    let f = frames.max(1.0);
    let n: Vec<f64> = PORT6.iter().map(|c| c.swap(0, Ordering::Relaxed) as f64 / f).collect();
    crate::xlog_info!(
        "renderer port 6 (XCL_PORT_GUARD=1) per frame: view faults under shared lock {:.1} ({:.2} ms waiting), \
         refreshes shared {:.1}, blocks left for a claim {:.1}, claim spins {:.0}",
        n[P6_FAULTS],
        n[P6_FAULT_WAIT_NS] / 1e6,
        n[P6_REFRESH_SHARED],
        n[P6_CLAIMS_MISSED],
        n[P6_CLAIM_SPINS],
    );
}

// ---------------------------------------------------------------------------
// Port 7: sharded journal notes
// ---------------------------------------------------------------------------

struct Shard {
    /// 4 KiB page address → last asking number that found it dirty.
    written: BTreeMap<usize, u64>,
}

impl Shard {
    const fn new() -> Shard {
        Shard { written: BTreeMap::new() }
    }

    fn note(&mut self, page: usize, parts: u16, now: u64) -> u64 {
        let before = self.written.get(&page).copied().unwrap_or(0);
        let next = before.max(now);
        if next > before {
            self.written.insert(page, next);
        }
        let _ = parts;
        before
    }
}

struct Notes {
    shards: [RwLock<Shard>; SHARDS],
}

impl Notes {
    const fn new() -> Notes {
        Notes { shards: [const { RwLock::new(Shard::new()) }; SHARDS] }
    }

    fn note_pages(&self, pages: &[(usize, u16)], now: u64) {
        for &(page, parts) in pages {
            let s = shard_of(page);
            let began = std::time::Instant::now();
            let mut g = match self.shards[s].try_write() {
                Ok(guard) => guard,
                Err(_) => {
                    port7_waited(P7_PART_WAITS, began);
                    self.shards[s].write().unwrap_or_else(|e| e.into_inner())
                }
            };
            g.note(page, parts, now);
        }
    }

    fn page_generation(&self, page: usize) -> u64 {
        self.shards[shard_of(page)]
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .written
            .get(&page)
            .copied()
            .unwrap_or(0)
    }
}

struct Journal {
    notes: Notes,
    /// Next asking number (1-based).
    asking: u64,
}

static JOURNAL: RwLock<Journal> = RwLock::new(Journal { notes: Notes::new(), asking: 0 });

static PORT7: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
const P7_ALONE: usize = 0;
const P7_ALONE_WAITS: usize = 1;
const P7_READ_WAITS: usize = 3;
const P7_SHARED_NOTES: usize = 5;
const P7_PART_WAITS: usize = 6;
const P7_SKIPPED: usize = 8;

fn port7_count(what: usize, n: u64) {
    PORT7[what].fetch_add(n, Ordering::Relaxed);
}

fn port7_waited(what: usize, began: std::time::Instant) {
    let ns = began.elapsed().as_nanos() as u64;
    port7_count(what, 1);
    port7_count(what + 1, ns);
}

fn next_asking() -> u64 {
    let mut j = JOURNAL.write().unwrap_or_else(|e| e.into_inner());
    j.asking += 1;
    j.asking
}

static LANDED_BLOCKS: AtomicU64 = AtomicU64::new(0);

/// Landing wrote title bytes in block `block` (`XCL_LAND_BESIDE` path).
pub(crate) fn note_landed_block(block: usize) {
    LANDED_BLOCKS.fetch_add(1, Ordering::Relaxed);
    if port::journal() {
        let page = block * BLOCK;
        let pages = (0..BLOCK).step_by(KERNEL_PAGE).map(|o| page + o).collect::<Vec<_>>();
        note_write_watch_dirty(&pages, KERNEL_PAGE);
    }
}

/// Note write-watch dirty pages after a scan (`refresh_once`).
pub(crate) fn note_write_watch_dirty(dirty: &[usize], page_size: usize) {
    if !port::journal() || dirty.is_empty() {
        return;
    }
    let step = page_size.max(KERNEL_PAGE);
    let parts_on = super::page_env::page_parts() && super::texture_env::take_parts();
    let _ = super::page_env::page_writes();
    let _ = super::page_env::open_one_view();
    let pages: Vec<(usize, u16)> = dirty
        .iter()
        .map(|&p| {
            if parts_on {
                let block = p & !(BLOCK - 1);
                let bit = ((p - block) / KERNEL_PAGE).min(PARTS - 1) as u16;
                (block, 1u16 << bit)
            } else {
                (p & !(step - 1), WHOLE)
            }
        })
        .collect();
    let now = {
        let began = std::time::Instant::now();
        let mut j = match JOURNAL.try_write() {
            Ok(g) => g,
            Err(_) => {
                port7_waited(P7_ALONE_WAITS, began);
                JOURNAL.write().unwrap_or_else(|e| e.into_inner())
            }
        };
        port7_count(P7_ALONE, 1);
        j.asking += 1;
        j.asking
    };
    {
        let began = std::time::Instant::now();
        let j = match JOURNAL.try_read() {
            Ok(g) => g,
            Err(_) => {
                port7_waited(P7_READ_WAITS, began);
                JOURNAL.read().unwrap_or_else(|e| e.into_inner())
            }
        };
        if pages.iter().all(|&(page, _)| j.notes.page_generation(page) >= now) {
            port7_count(P7_SKIPPED, 1);
            return;
        }
    }
    let mask = shards_of_pages(pages.iter().map(|p| p.0));
    let began = std::time::Instant::now();
    let j = JOURNAL.read().unwrap_or_else(|e| e.into_inner());
    port7_waited(P7_PART_WAITS, began);
    j.notes.note_pages(&pages, now);
    port7_count(P7_SHARED_NOTES, 1);
    let _ = mask;
}

/// Generation of a 4 KiB page in the journal (0 = never noted).
pub(crate) fn page_generation(page: usize) -> u64 {
    if !port::journal() {
        return 0;
    }
    JOURNAL.read().unwrap_or_else(|e| e.into_inner()).notes.page_generation(page)
}

pub(crate) struct Port7Counts(pub [u64; 10]);

pub(crate) fn port7_counts() -> Port7Counts {
    Port7Counts(std::array::from_fn(|i| PORT7[i].swap(0, Ordering::Relaxed)))
}

impl Port7Counts {
    pub(crate) fn per_frame(&self, frames: f64) -> String {
        let f = frames.max(1.0);
        let n = |i: usize| self.0[i] as f64 / f;
        let ms = |i: usize| self.0[i] as f64 / 1e6 / f;
        format!(
            "journal alone {:.1} ({:.2} ms), shared read {:.1} ({:.2} ms), shared notes {:.1}, part waits {:.1} ({:.2} ms), skipped {:.1}",
            n(P7_ALONE_WAITS),
            ms(P7_ALONE_WAITS + 1),
            n(P7_READ_WAITS),
            ms(P7_READ_WAITS + 1),
            n(P7_SHARED_NOTES),
            n(P7_PART_WAITS),
            ms(P7_PART_WAITS + 1),
            n(P7_SKIPPED),
        )
    }
}

/// Heartbeat for `XCL_ASK_EARLY` background thread (journal path stays warm).
pub(crate) fn tick_early_ask() {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n == 0 {
        super::ask_env::on_refresh_start();
        super::keep_env::note_boot();
        super::page_env::note_boot();
        super::pop_env::note_boot();
        super::read_env::note_boot();
    }
}

pub fn guard_ask_always() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_GUARD_ASK").is_ok_and(|v| v.trim() == "1"))
}

pub fn guard_hot_ms() -> u32 {
    static MS: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MS.get_or_init(|| {
        std::env::var("XCL_GUARD_HOT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
            .min(1000)
    })
}

pub fn guard_rest_us() -> u32 {
    static US: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *US.get_or_init(|| {
        std::env::var("XCL_GUARD_REST")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|&us| us > 1)
            .unwrap_or(3000)
    })
}

pub fn guard_holes_extended() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_GUARD_HOLES").is_none_or(|v| v != "0"))
}

pub(crate) fn port7_line(frames: f64) {
    if !port::journal() {
        return;
    }
    let c = port7_counts();
    crate::xlog_info!(
        "renderer port 7 (XCL_PORT_JOURNAL=1) per frame: {}",
        c.per_frame(frames),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_mask_covers_pages() {
        assert_eq!(shards_of_pages([0usize, 1 << SHARD_SHIFT]), 0b11);
    }

    #[test]
    fn claims_release_on_drop() {
        let a = claim_blocks(&[1, 2]);
        assert!(!BUSY.try_set(1));
        drop(a);
        assert!(BUSY.try_set(1));
        BUSY.clear(1);
    }
}