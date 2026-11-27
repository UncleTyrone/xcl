//! Async GPU write landing (`XCL_RELAY=1` or `2`). Default off.
//!
//! NVIDIA gap: worker uses synchronous sparse readback (not the default async rbq path).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::ID3D12CommandQueue;

static QUEUE_ORDER: AtomicU64 = AtomicU64::new(0);
static IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static LANDED_ORDER: AtomicU64 = AtomicU64::new(0);

static STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

struct Job {
    queue: usize,
    ranges: Vec<(usize, usize)>,
    order: u64,
}

static JOBS: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
static MORE: Condvar = Condvar::new();
static WORKER: OnceLock<()> = OnceLock::new();

/// `0` off, `1` first stage, `2` exact (default stage when enabled).
pub fn stage() -> u32 {
    super::port::relay_stage()
}

pub fn on() -> bool {
    stage() > 0
}

/// Second relay stage: submission may wait on landed writes (`XCL_RELAY=2` only).
pub fn exact() -> bool {
    stage() == 2
}

pub fn relay_copy() -> bool {
    super::port::relay_copy()
}

fn ensure_worker() {
    WORKER.get_or_init(|| {
        let _ = std::thread::Builder::new().name("xcl-relay".into()).spawn(|| loop {
            let job = {
                let mut q = JOBS.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(j) = q.pop_front() {
                        break j;
                    }
                    q = MORE.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            STATS[0].fetch_add(1, Ordering::Relaxed);
            let qptr = job.queue as *mut c_void;
            // SAFETY: queue handle came from title submit path; same contract as sync readback.
            unsafe {
                if let Some(queue) = ID3D12CommandQueue::from_raw_borrowed(&qptr) {
                    super::sparse::readback_after_execute_sync(queue, &job.ranges);
                }
            }
            LANDED_ORDER.store(job.order, Ordering::Release);
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            MORE.notify_all();
        });
        let st = stage();
        crate::xlog_info!(
            "D3D12.X: relay worker (XCL_RELAY={st}{}{}{})",
            if exact() { ", exact" } else { "" },
            if relay_copy() { ", copy queue fetch" } else { "" },
            if super::diag_env::fetch_check() { ", XCL_FETCH_CHECK" } else { "" },
        );
    });
}

pub fn next_order() -> u64 {
    QUEUE_ORDER.fetch_add(1, Ordering::AcqRel) + 1
}

pub fn submit_order() -> u64 {
    QUEUE_ORDER.load(Ordering::Acquire)
}

pub fn landed_order() -> u64 {
    LANDED_ORDER.load(Ordering::Acquire)
}

/// Queue sparse readback/land on the relay thread when [`on`]; returns `true` if deferred.
pub unsafe fn defer_sparse_readback(queue: *mut c_void, ranges: &[(usize, usize)]) -> bool {
    if !on() || ranges.is_empty() || queue.is_null() {
        return false;
    }
    ensure_worker();
    let order = next_order();
    IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
    {
        let mut q = JOBS.lock().unwrap_or_else(|e| e.into_inner());
        q.push_back(Job {
            queue: queue as usize,
            ranges: ranges.to_vec(),
            order,
        });
    }
    MORE.notify_one();
    true
}

pub fn wait_landed_through(through: u64) {
    if !exact() {
        return;
    }
    let began = std::time::Instant::now();
    let mut g = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    while LANDED_ORDER.load(Ordering::Acquire) < through || IN_FLIGHT.load(Ordering::Acquire) > 0 {
        g = MORE.wait(g).unwrap_or_else(|e| e.into_inner());
        if began.elapsed().as_secs() > 30 {
            crate::xlog_warn!("D3D12.X: relay wait_landed_through timed out (order {through})");
            break;
        }
    }
}

pub fn wait_before_submit() {
    if !exact() {
        return;
    }
    let target = QUEUE_ORDER.load(Ordering::Acquire);
    if target > 0 {
        wait_landed_through(target);
    }
}

pub fn note_frame(frames: f64) {
    if !on() || frames <= 0.0 {
        return;
    }
    let landings = STATS[0].swap(0, Ordering::Relaxed) as f64 / frames;
    if landings > 0.0 {
        crate::xlog_info!("D3D12.X: relay per frame (XCL_RELAY={}): {:.1} landings", stage(), landings);
    }
    super::land::log_stats(frames);
    super::card_steps::log_stats(frames);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_off_by_default() {
        assert_eq!(stage(), 0);
        assert!(!on());
    }
}