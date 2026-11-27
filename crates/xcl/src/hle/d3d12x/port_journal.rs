//! Belobug renderer port 7 (`XCL_PORT_JOURNAL`): kick a descriptor heap-sync round at submit and
//! wait for it while the submission is prepared. Wired from `ExecuteCommandLists`; reimplemented
//! for XCL (no victory-gpu paste).

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Condvar, Mutex, Once};
use std::time::Instant;

use super::port;

pub(crate) fn on() -> bool {
    port::journal()
}

/// With rounds `asked` requested and `done` finished: the next round number and whether to notify.
pub(crate) fn round_after(asked: u64, done: u64) -> (u64, bool) {
    if asked >= done + 2 {
        (asked, false)
    } else {
        (asked + 1, true)
    }
}

thread_local! {
    static MARK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(crate) fn set_mark(mark: u64) {
    MARK.set(mark);
}

fn mark() -> u64 {
    MARK.get()
}

static PREP_FLOOR: AtomicU64 = AtomicU64::new(0);

/// A prepare step may have written title descriptor memory; later submits need a later round.
pub(crate) fn prep_wrote() {
    PREP_FLOOR.fetch_max(round_mark(), Relaxed);
    STATS[PREP_WRITES].fetch_add(1, Relaxed);
}

pub(crate) fn needed() -> u64 {
    if !on() {
        return 0;
    }
    match mark() {
        0 => {
            STATS[NO_MARK].fetch_add(1, Relaxed);
            0
        }
        m => m.max(PREP_FLOOR.load(Relaxed)),
    }
}

pub(crate) static STATS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
pub(crate) const ROUNDS_ASKED: usize = 0;
pub(crate) const EARLY_ROUNDS: usize = 1;
const PREP_WRITES: usize = 2;
const HEAPS_NS_ON: usize = 3;
const HEAPS_NS_OFF: usize = 4;
const HEAPS_ON: usize = 5;
const HEAPS_OFF: usize = 6;
const NO_MARK: usize = 7;
pub(crate) const LATE_ROUNDS: usize = 8;

pub(crate) fn heaps_waited(ns: u64, journal_on: bool) {
    let (time, count) = if journal_on { (HEAPS_NS_ON, HEAPS_ON) } else { (HEAPS_NS_OFF, HEAPS_OFF) };
    STATS[time].fetch_add(ns, Relaxed);
    STATS[count].fetch_add(1, Relaxed);
}

static SYNC_ROUNDS: (Mutex<(u64, u64)>, Condvar) = (Mutex::new((0, 0)), Condvar::new());

/// One round of whatever syncs descriptor heaps (placeholder until full mirror sync lands).
fn sync_heap_records_round() {
    // Port 2 records land in title memory; mirroring into PC heaps is still on the submit path.
    let _ = super::descriptor_sites::stores_hooked();
}

fn sync_aside() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        on() && std::env::var_os("XCL_HEAP_SYNC_ASIDE").is_none_or(|v| v != "0")
    })
}

fn sync_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_HEAP_SYNC").is_none_or(|v| v != "0"))
}

#[must_use = "the submit lane waits for this round until dropped"]
pub(crate) struct SyncedHeaps(Option<u64>);

/// Ask for the next heap-sync round at submit (port 7); wait ends when this is dropped.
pub(crate) fn begin_at_submit() -> SyncedHeaps {
    if !on() || !sync_on() {
        return SyncedHeaps(None);
    }
    if !sync_aside() {
        sync_heap_records_round();
        return SyncedHeaps(None);
    }
    start_sync_thread();
    let mark = round_after_now();
    if mark == 0 {
        return SyncedHeaps(None);
    }
    set_mark(mark);
    begin_sync_heaps_after(mark)
}

fn start_sync_thread() {
    static STARTED: Once = Once::new();
    static OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    STARTED.call_once(|| {
        let made = std::thread::Builder::new()
            .name("xcl-heap-sync".into())
            .spawn(|| {
                loop {
                    let early = super::render_env::heap_sync_early();
                    let mut r = SYNC_ROUNDS.0.lock().unwrap_or_else(|e| e.into_inner());
                    while r.0 == r.1 {
                        match early {
                            Some(d) => {
                                let (guard, wait) = SYNC_ROUNDS.1.wait_timeout(r, d).unwrap_or_else(|e| e.into_inner());
                                r = guard;
                                if wait.timed_out() && r.0 == r.1 {
                                    // Early round while the submit lane has not asked yet.
                                    r.0 = r.0.max(1);
                                    STATS[EARLY_ROUNDS].fetch_add(1, Relaxed);
                                    break;
                                }
                            }
                            None => {
                                r = SYNC_ROUNDS.1.wait(r).unwrap_or_else(|e| e.into_inner());
                            }
                        }
                    }
                    drop(r);
                    let _ = std::panic::catch_unwind(sync_heap_records_round);
                    let mut r = SYNC_ROUNDS.0.lock().unwrap_or_else(|e| e.into_inner());
                    r.1 += 1;
                    drop(r);
                    SYNC_ROUNDS.1.notify_all();
                }
            });
        OK.store(made.is_ok(), std::sync::atomic::Ordering::Release);
    });
    if !OK.load(std::sync::atomic::Ordering::Acquire) {
        sync_heap_records_round();
    }
}

pub(crate) fn round_after_now() -> u64 {
    if !on() || !sync_on() || !sync_aside() {
        return 0;
    }
    start_sync_thread();
    let mut rounds = SYNC_ROUNDS.0.lock().unwrap_or_else(|e| e.into_inner());
    if rounds.1 == 0 && rounds.0 == 0 {
        return 0;
    }
    let (round, ask) = round_after(rounds.0, rounds.1);
    if ask {
        rounds.0 = round;
        drop(rounds);
        SYNC_ROUNDS.1.notify_all();
        STATS[ROUNDS_ASKED].fetch_add(1, Relaxed);
    } else {
        drop(rounds);
    }
    round
}

pub(crate) fn round_mark() -> u64 {
    if !sync_on() || !sync_aside() {
        return 0;
    }
    let rounds = SYNC_ROUNDS.0.lock().unwrap_or_else(|e| e.into_inner());
    if rounds.1 == 0 && rounds.0 == 0 {
        return 0;
    }
    rounds.0 + 1
}

fn begin_sync_heaps_after(mark: u64) -> SyncedHeaps {
    if mark == 0 || !sync_aside() {
        return SyncedHeaps(None);
    }
    let mut rounds = SYNC_ROUNDS.0.lock().unwrap_or_else(|e| e.into_inner());
    if rounds.1 == 0 && rounds.0 == 0 {
        drop(rounds);
        return SyncedHeaps(None);
    }
    let asked = rounds.0 < mark;
    rounds.0 = rounds.0.max(mark);
    drop(rounds);
    if asked {
        SYNC_ROUNDS.1.notify_all();
        STATS[LATE_ROUNDS].fetch_add(1, Relaxed);
    } else {
        STATS[EARLY_ROUNDS].fetch_add(1, Relaxed);
    }
    SyncedHeaps(Some(mark))
}

impl Drop for SyncedHeaps {
    fn drop(&mut self) {
        let Some(mine) = self.0 else { return };
        let began = Instant::now();
        let mut rounds = SYNC_ROUNDS.0.lock().unwrap_or_else(|e| e.into_inner());
        while rounds.1 < mine {
            rounds = SYNC_ROUNDS.1.wait(rounds).unwrap_or_else(|e| e.into_inner());
        }
        heaps_waited(began.elapsed().as_nanos() as u64, on());
    }
}

pub(crate) fn line(frames: f64) {
    if !on() {
        return;
    }
    let s: Vec<f64> = STATS.iter().map(|x| x.swap(0, Relaxed) as f64).collect();
    let journal = super::watch::port7_counts();
    let f = frames.max(1.0);
    let (on_ms, off_ms) = (s[HEAPS_NS_ON] / 1e6 / f, s[HEAPS_NS_OFF] / 1e6 / f);
    crate::xlog_info!(
        "renderer port 7 (XCL_PORT_JOURNAL=1) per frame: heaps: lane waited {on_ms:.2} ms ({:.1} prepares) with the switch, \
         {off_ms:.2} ms ({:.1}) without; {:.1} rounds asked at submit, {:.1} early, {:.1} late, {:.1} no mark, {:.1} prep wrote; {}",
        s[HEAPS_ON] / f,
        s[HEAPS_OFF] / f,
        s[ROUNDS_ASKED] / f,
        s[EARLY_ROUNDS] / f,
        s[LATE_ROUNDS] / f,
        s[NO_MARK] / f,
        s[PREP_WRITES] / f,
        journal.per_frame(f),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_after_matches_belobug_rules() {
        assert_eq!(round_after(0, 0), (1, true));
        assert_eq!(round_after(3, 2), (4, true));
        assert_eq!(round_after(5, 2), (5, false));
        assert_eq!(round_after(7, 7), (8, true));
    }
}