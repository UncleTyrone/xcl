//! First uploads of pending textures before the submit needs them (`XCL_UPLOAD_AHEAD`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::ID3D12Fence;

use super::texture_policy;

/// A copy-queue fence value title queues wait for before sampling ahead-uploaded textures.
pub(super) struct Sent {
    pub fence: ID3D12Fence,
    pub value: u64,
}

struct AheadFence {
    fence: ID3D12Fence,
    value: u64,
    waited: std::collections::BTreeMap<usize, u64>,
}

static PENDING: (Mutex<VecDeque<usize>>, Condvar) = (Mutex::new(VecDeque::new()), Condvar::new());
static FENCE: Mutex<Option<AheadFence>> = Mutex::new(None);
static WORKER: OnceLock<()> = OnceLock::new();
static STATS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3]; // queued, done, turned down

pub(super) fn on() -> bool {
    texture_policy::upload_ahead()
}

pub(super) fn offer(res: usize) {
    if !on() {
        return;
    }
    if super::residency::on()
        && (super::residency::upload_refused() || !super::residency::allow_staging_growth(false))
    {
        STATS[2].fetch_add(1, Ordering::Relaxed);
        return;
    }
    ensure_worker();
    let mut q = PENDING.0.lock().unwrap();
    if !q.contains(&res) {
        q.push_back(res);
        STATS[0].fetch_add(1, Ordering::Relaxed);
    }
    PENDING.1.notify_one();
}

/// Before title lists run, wait for ahead uploads this queue must follow.
pub(super) unsafe fn wait_for(title: &windows::Win32::Graphics::Direct3D12::ID3D12CommandQueue) {
    if !on() {
        return;
    }
    let key = title.as_raw() as usize;
    let mut g = FENCE.lock().unwrap();
    let Some(a) = g.as_mut() else {
        return;
    };
    if a.value == 0 || a.waited.get(&key).copied().unwrap_or(0) >= a.value {
        return;
    }
    if a.fence.GetCompletedValue() < a.value {
        let _ = title.Wait(&a.fence, a.value);
        super::list_x::note_internal_wait(key, "texture uploads ahead", &a.fence, a.value);
    }
    a.waited.insert(key, a.value);
}

pub(super) fn record_sent(sent: &Sent) {
    if !on() {
        return;
    }
    let mut g = FENCE.lock().unwrap();
    let a = g.get_or_insert(AheadFence {
        fence: sent.fence.clone(),
        value: 0,
        waited: std::collections::BTreeMap::new(),
    });
    if sent.value > a.value {
        a.value = sent.value;
        a.fence = sent.fence.clone();
    }
}

fn ensure_worker() {
    WORKER.get_or_init(|| {
        let _ = std::thread::Builder::new().name("xcl-texture-ahead".into()).spawn(worker);
    });
}

fn worker() {
    loop {
        let res = {
            let mut q = PENDING.0.lock().unwrap();
            while q.is_empty() {
                q = PENDING.1.wait(q).unwrap();
            }
            q.pop_front().unwrap()
        };
        let _ = super::title_textures::own_work(|| unsafe {
            match super::title_textures::upload_if_pending(res) {
                Ok(Some(sent)) => {
                    record_sent(&sent);
                    STATS[1].fetch_add(1, Ordering::Relaxed);
                }
                Ok(None) => {
                    STATS[2].fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    STATS[2].fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }
}