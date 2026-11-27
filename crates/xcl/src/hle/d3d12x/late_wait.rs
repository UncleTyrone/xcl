//! Late GPU flights (`XCL_LATE_WAIT`, cool_fox `VICTORY_LATE_WAIT` 0/1/2/3). **Default off.**
//!
//! Behavioral port (GPL-2 clean; no CGBJ paste). Overlaps with relay + async submit when enabled.
//!
//! | Stage | Behavior |
//! |-------|----------|
//! | 0 | Unchanged sync path. |
//! | 1 | Skip per-chunk serial GPU wait when under [`max_in_flight_chunks`]. |
//! | 2 | Stage 1 + shorten resubmit fence waits when under cap. |
//! | 3 | Stage 2 + present may queue ahead with [`super::submit::present_may_queue`]. |

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Fence, D3D12_FENCE_FLAG_NONE};

use crate::gpu::fence;

static LOGGED: OnceLock<()> = OnceLock::new();

thread_local! {
    static CHUNK_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn relay_env() -> u32 {
    match std::env::var("XCL_RELAY").ok().as_deref().map(str::trim) {
        Some("1") => 1,
        Some("2") => 2,
        _ => 0,
    }
}

/// Raw env (ignores `XCL_GPU_TIMING` force-off).
pub fn raw_stage() -> u32 {
    static S: OnceLock<u32> = OnceLock::new();
    *S.get_or_init(|| match std::env::var("XCL_LATE_WAIT").ok().as_deref().map(str::trim) {
        None | Some("") | Some("0") | Some("off") => 0,
        Some("1") => 1,
        Some("2") => 2,
        Some("3") => 3,
        Some(bad) => {
            crate::xlog_warn!("D3D12.X: XCL_LATE_WAIT={bad:?} invalid (use 0–3); treating as off");
            0
        }
    })
}

/// `XCL_GPU_TIMING` forces synchronous timing (late flights off), matching cool_fox.
pub fn gpu_timing_forces_sync() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_GPU_TIMING").is_some())
}

/// Effective stage: 0 when unset, forced 0 with `XCL_GPU_TIMING`.
pub fn stage() -> u32 {
    let s = raw_stage();
    if s == 0 {
        return 0;
    }
    if gpu_timing_forces_sync() {
        return 0;
    }
    LOGGED.get_or_init(|| {
        crate::xlog_info!(
            "D3D12.X: XCL_LATE_WAIT={s} (cap {} in-flight chunks, BOP settle {})",
            max_in_flight_chunks(),
            settle_bottom_writes(),
        );
        if s >= 2 && relay_env() != 2 {
            crate::xlog_warn!(
                "D3D12.X: XCL_LATE_WAIT>={s} pairs with XCL_RELAY=2 for land overlap (relay is {:?})",
                match relay_env() {
                    0 => "off",
                    1 => "1",
                    _ => "?",
                }
            );
        }
        if s >= 3 && !super::port::async_submit() {
            crate::xlog_warn!("D3D12.X: XCL_LATE_WAIT=3 pairs with XCL_ASYNC_SUBMIT=1 for present-as-flight");
        }
    });
    s
}

pub fn on() -> bool {
    stage() > 0
}

pub fn present_ahead() -> bool {
    stage() >= 3
}

/// `XCL_LATE_WAIT_LISTS` (default 256).
pub fn max_in_flight_chunks() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("XCL_LATE_WAIT_LISTS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(256)
            .max(1)
    })
}

/// `XCL_LATE_WAIT_WRITES`: on unless `=0`.
pub fn settle_bottom_writes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_LATE_WAIT_WRITES").is_none_or(|v| v != "0"))
}

// ---- in-flight chunk / flight tracking -------------------------------------------------------

#[derive(Clone)]
struct OpenChunk {
    id: u64,
    queue: usize,
    bop_pending: u32,
    write64: Vec<(u64, u64)>,
    relay_through: u64,
    fence_value: u64,
    fence: Option<ID3D12Fence>,
}

static FLIGHT_SEQ: AtomicU64 = AtomicU64::new(0);
static OPEN: Mutex<VecDeque<OpenChunk>> = Mutex::new(VecDeque::new());
static PROMISED_W64: OnceLock<Mutex<HashMap<u64, u64>>> = OnceLock::new();

fn promised_w64() -> &'static Mutex<HashMap<u64, u64>> {
    PROMISED_W64.get_or_init(|| Mutex::new(HashMap::new()))
}

static SKIPPED_SERIAL: AtomicU64 = AtomicU64::new(0);
static SHORT_RESUBMIT: AtomicU64 = AtomicU64::new(0);
static LATE_MET_HITS: AtomicU64 = AtomicU64::new(0);
static RELAY_RELAXED: AtomicU64 = AtomicU64::new(0);
static LATE_WAITED: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5]; // 0,1,2-3,4-7,8+

fn bucket_late_waited(depth: usize) {
    let i = match depth {
        0 => 0,
        1 => 1,
        2..=3 => 2,
        4..=7 => 3,
        _ => 4,
    };
    LATE_WAITED[i].fetch_add(1, Ordering::Relaxed);
}

fn refresh_promised_w64() {
    let open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let mut prom = promised_w64().lock().unwrap_or_else(|e| e.into_inner());
    prom.clear();
    for ch in open.iter() {
        for &(va, v) in &ch.write64 {
            prom.entry(va).and_modify(|m| *m = (*m).max(v)).or_insert(v);
        }
    }
}

/// Id of the chunk currently being submitted on this thread (0 if none / late-wait off).
pub fn current_chunk_id() -> u64 {
    CHUNK_ID.get()
}

pub fn chunks_ahead() -> u64 {
    OPEN.lock().map(|q| q.len() as u64).unwrap_or(0)
}

pub fn may_send_ahead() -> bool {
    on() && chunks_ahead() < max_in_flight_chunks() as u64
}

/// Begin a submit chunk on `queue`; returns a flight id (0 when late-wait off).
pub fn begin_chunk(queue: usize, relay_through: u64) -> u64 {
    if !on() {
        CHUNK_ID.set(0);
        return 0;
    }
    let id = FLIGHT_SEQ.fetch_add(1, Ordering::AcqRel) + 1;
    CHUNK_ID.set(id);
    {
        let mut q = OPEN.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= max_in_flight_chunks() as usize {
            poll_landed_chunks();
        }
        q.push_back(OpenChunk {
            id,
            queue,
            bop_pending: 0,
            write64: Vec::new(),
            relay_through,
            fence_value: 0,
            fence: None,
        });
        bucket_late_waited(q.len());
    }
    id
}

/// Attach the alloc-pool fence for the chunk just executed on `queue`.
pub unsafe fn attach_queue_fence(queue: *mut c_void, fence: &ID3D12Fence, value: u64) {
    if !on() {
        return;
    }
    let id = CHUNK_ID.get();
    if id == 0 {
        return;
    }
    let key = queue as usize;
    let mut q = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ch) = q.iter_mut().rev().find(|c| c.id == id && c.queue == key) {
        ch.fence = Some(fence.clone());
        ch.fence_value = value;
    }
}

/// Record GPU `Write64` signals and BOP jobs for the open chunk.
pub fn note_chunk_payload(write64: &[(u64, u64)], bop_jobs: u32) {
    if !on() {
        return;
    }
    let id = CHUNK_ID.get();
    if id == 0 {
        return;
    }
    let mut q = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ch) = q.iter_mut().rev().find(|c| c.id == id) else {
        return;
    };
    ch.write64.extend_from_slice(write64);
    ch.bop_pending = ch.bop_pending.saturating_add(bop_jobs);
    if !write64.is_empty() {
        refresh_promised_w64();
    }
}

pub fn note_bop_settled(flight_id: u64, n: u32) {
    if !on() || flight_id == 0 || n == 0 {
        return;
    }
    let mut q = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ch) = q.iter_mut().find(|c| c.id == flight_id) {
        ch.bop_pending = ch.bop_pending.saturating_sub(n);
    }
}

/// End chunk bookkeeping on this thread.
pub fn end_chunk() {
    CHUNK_ID.set(0);
}

/// After driver execute (compat): bump tracking when `begin_chunk` was not used.
pub fn note_chunk_sent() {
    if on() && CHUNK_ID.get() == 0 {
        begin_chunk(0, 0);
        end_chunk();
    }
}

/// Poll completed queue fences and drop landed chunks.
pub fn poll_landed_chunks() {
    if !on() {
        return;
    }
    let mut q = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let mut changed_w64 = false;
    while let Some(front) = q.front() {
        let gpu_done = match (&front.fence, front.fence_value) {
            (Some(f), v) if v > 0 => unsafe { f.GetCompletedValue() >= v },
            (None, 0) | (_, 0) if !settle_bottom_writes() => true,
            _ => false,
        };
        let bop_clear = front.bop_pending == 0 || !settle_bottom_writes();
        if !gpu_done || !bop_clear {
            break;
        }
        q.pop_front();
        changed_w64 = true;
    }
    if changed_w64 {
        refresh_promised_w64();
    }
}

/// Present / frame boundary: drain landed work (does not force-clear the whole cap).
pub fn note_present_boundary() {
    poll_landed_chunks();
}

/// When true, AMD relay [`super::relay::wait_before_submit`] may be skipped (stage ≥ 1 under cap).
pub fn relax_relay_wait_before_submit() -> bool {
    let ok = stage() >= 1 && may_send_ahead();
    if ok {
        RELAY_RELAXED.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// BOP settle: prior chunks with pending CPU bottom-of-pipe work must finish before dependent
/// GPU work when [`settle_bottom_writes`] is on.
pub unsafe fn settle_bop_before_submit(queue: *mut c_void) {
    if !on() || !settle_bottom_writes() {
        return;
    }
    poll_landed_chunks();
    let began = std::time::Instant::now();
    loop {
        let (wait_fence, wait_val, bop) = {
            let q = OPEN.lock().unwrap_or_else(|e| e.into_inner());
            let Some(front) = q.front() else { break };
            if front.bop_pending == 0 {
                break;
            }
            (front.fence.clone(), front.fence_value, front.bop_pending)
        };
        if bop == 0 {
            break;
        }
        if let (Some(f), v) = (wait_fence, wait_val) {
            if v > 0 && unsafe { f.GetCompletedValue() < v } {
                if began.elapsed() > Duration::from_millis(50) {
                    break;
                }
                std::thread::sleep(Duration::from_micros(200));
                continue;
            }
        }
        // GPU done; give cpu-writes worker a slice to drain BOP for this flight.
        if began.elapsed() > Duration::from_millis(500) {
            break;
        }
        std::thread::sleep(Duration::from_micros(500));
        poll_landed_chunks();
        let done = OPEN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .front()
            .map_or(true, |f| f.bop_pending == 0);
        if done {
            break;
        }
    }
    let _ = queue;
}

/// Value-wait helper: true when an in-flight flight will publish `value` at `va` (late_met).
pub fn memory_wait_late_met(va: u64, comparison: u32, want: u64, mask: u64) -> bool {
    if !on() {
        return false;
    }
    let prom = promised_w64().lock().unwrap_or_else(|e| e.into_inner());
    let Some(&v) = prom.get(&va) else {
        return false;
    };
    let lhs = v & mask;
    let met = match comparison {
        7 => lhs >= want, // GREATER_EQUAL
        4 => lhs <= want,
        3 => lhs == want,
        8 => true,
        _ => false,
    };
    if met {
        LATE_MET_HITS.fetch_add(1, Ordering::Relaxed);
    }
    met
}

/// Skip [`super::session::serial_wait_after_submit`] when late flights are on and under cap.
pub fn skip_serial_wait() -> bool {
    if on() && may_send_ahead() {
        SKIPPED_SERIAL.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Override for [`super::alloc_pool::resubmit_wait`]; `None` = use default env/ms.
pub fn resubmit_wait_override() -> Option<Duration> {
    if stage() < 2 || !may_send_ahead() {
        return None;
    }
    SHORT_RESUBMIT.fetch_add(1, Ordering::Relaxed);
    Some(Duration::ZERO)
}

// ---- present-as-flight (stage 3 + async submit) ---------------------------------------------

pub fn present_flight_begin() {
    if !present_ahead() || !super::port::async_submit() {
        return;
    }
    super::submit::wait_present_room();
    super::submit::note_present_queued();
}

pub fn present_flight_end() {
    if !present_ahead() || !super::port::async_submit() {
        return;
    }
    super::submit::note_submit_done();
}

pub fn present_may_skip_pipe_wait() -> bool {
    present_ahead() && super::port::async_submit() && super::submit::present_behind() < super::submit::frames_cap() as u64
}

// ---- serial submit wait (stage 1: skip when under cap) --------------------------------------

struct SerialFence {
    fence: ID3D12Fence,
    value: u64,
}

static SERIAL: Mutex<Option<HashMap<usize, SerialFence>>> = Mutex::new(None);

/// After each chunk when `XCL_SUBMIT_SERIAL` is on: wait until the queue finishes it unless
/// late-wait stage 1+ is under the in-flight cap.
pub unsafe fn serial_wait_after_submit(queue: *mut c_void) {
    if !super::port::submit_serial() || queue.is_null() || skip_serial_wait() {
        return;
    }
    let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&queue) else {
        return;
    };
    let mut dev: Option<ID3D12Device> = None;
    if q.GetDevice(&mut dev).is_err() {
        return;
    }
    let Some(dev) = dev else {
        return;
    };
    let key = queue as usize;
    let (fence, value) = {
        let mut g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let map = g.get_or_insert_with(HashMap::new);
        let entry = map.entry(key).or_insert_with(|| SerialFence {
            fence: dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).expect("serial fence"),
            value: 0,
        });
        entry.value += 1;
        let v = entry.value;
        if q.Signal(&entry.fence, v).is_err() {
            return;
        }
        (entry.fence.clone(), v)
    };
    if let Err(e) = fence::wait(&fence, value, &dev, None, 30_000) {
        crate::xlog_warn!("D3D12.X: serial submit wait timed out or failed ({e})");
    }
}

// ---- logging (rate-limited ~2 s like fill_ahead) --------------------------------------------

pub fn log_frame_stats(frames: f64) {
    if !on() || frames <= 0.0 {
        return;
    }
    let serial = SKIPPED_SERIAL.swap(0, Ordering::Relaxed) as f64 / frames;
    let resub = SHORT_RESUBMIT.swap(0, Ordering::Relaxed) as f64 / frames;
    if serial > 0.0 || resub > 0.0 {
        crate::xlog_info!(
            "D3D12.X: late-wait XCL_LATE_WAIT={} per frame: {:.2} serial waits skipped, {:.2} short resubmit waits",
            stage(),
            serial,
            resub,
        );
    }
    log_overlap_line(frames);
}

pub fn log_overlap_line(frames: f64) {
    if !on() || frames <= 0.0 {
        return;
    }
    static ACC_FRAMES: AtomicU64 = AtomicU64::new(0);
    let milli = (frames * 1000.0).round().max(1.0) as u64;
    ACC_FRAMES.fetch_add(milli, Ordering::Relaxed);

    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let now = std::time::Instant::now();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let due = match *last {
        None => {
            *last = Some(now);
            false
        }
        Some(t) if now.duration_since(t) >= Duration::from_secs(2) => {
            *last = Some(now);
            true
        }
        Some(_) => false,
    };
    if !due {
        return;
    }

    let window = ACC_FRAMES.swap(0, Ordering::Relaxed) as f64 / 1000.0;
    if window <= 0.0 {
        return;
    }
    let ahead = chunks_ahead();
    let late_met = LATE_MET_HITS.swap(0, Ordering::Relaxed) as f64 / window;
    let relay = RELAY_RELAXED.swap(0, Ordering::Relaxed) as f64 / window;
    let hist: Vec<u64> = LATE_WAITED.iter().map(|c| c.swap(0, Ordering::Relaxed)).collect();
    if ahead == 0 && late_met == 0.0 && relay == 0.0 && hist.iter().all(|&h| h == 0) {
        return;
    }
    crate::xlog_info!(
        "D3D12.X: LATE_WAITED (2 s): {:.0} chunks ahead; {:.1} late_met waits/frame; {:.1} relay relax/frame; depth hist [{}, {}, {}, {}, {}] (0/1/2-3/4-7/8+)",
        ahead,
        late_met,
        relay,
        hist[0],
        hist[1],
        hist[2],
        hist[3],
        hist[4],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_wait_off_by_default() {
        if std::env::var_os("XCL_LATE_WAIT").is_none() {
            assert_eq!(raw_stage(), 0);
            assert!(!on());
        }
    }

    #[test]
    fn lists_cap_defaults() {
        if std::env::var_os("XCL_LATE_WAIT_LISTS").is_none() {
            assert_eq!(max_in_flight_chunks(), 256);
        }
    }
}