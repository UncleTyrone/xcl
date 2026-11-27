//! Heap buffer-view pass beside list recording (`XCL_HEAP_PASS_ASIDE`, default on).
//!
//! Runs refresh discovery on a dedicated worker so the recording thread can
//! continue; [`Coming::wait`] joins before the list closes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::super::render_env;

pub(crate) fn on() -> bool {
    render_env::heap_pass_aside()
}

/// Host copy refresh: (resource key, from, to) — opaque to the worker.
pub(crate) type Refresh = (usize, u64, u64);

/// What is being found, until it is.
pub(crate) struct Coming(Mutex<Option<Vec<Refresh>>>, std::sync::Condvar);

pub(crate) static WAITED: AtomicU64 = AtomicU64::new(0);

impl Coming {
    pub(crate) fn wait(&self) -> Vec<Refresh> {
        let began = std::time::Instant::now();
        let _span = super::timeline::span("list close waits for heap views");
        let mut found = self.0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(found) = found.take() {
                WAITED.fetch_add(began.elapsed().as_micros() as u64, Ordering::Relaxed);
                return found;
            }
            found = self.1.wait(found).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn found(&self, what: Vec<Refresh>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(what);
        self.1.notify_all();
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// `work` begins on the aside thread (or inline if the latch is off / thread failed).
pub(crate) fn begin(work: impl FnOnce() -> Vec<Refresh> + Send + 'static) -> std::sync::Arc<Coming> {
    let coming = std::sync::Arc::new(Coming(Mutex::new(None), std::sync::Condvar::new()));
    if !on() {
        coming.found(work());
        return coming;
    }
    static THREAD: OnceLock<Option<Mutex<std::sync::mpsc::Sender<Job>>>> = OnceLock::new();
    let told = coming.clone();
    let job: Job = Box::new(move || {
        let _aside = super::timeline::span("beside: heap views");
        let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_default();
        told.found(found);
    });
    let thread = THREAD.get_or_init(|| {
        let (send, jobs) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("xcl-aside".into())
            .spawn(move || {
                jobs.into_iter().for_each(|job| job());
            })
            .ok()
            .map(|_| Mutex::new(send))
    });
    match thread {
        Some(send) => {
            if let Err(job) = send.lock().unwrap_or_else(|e| e.into_inner()).send(job) {
                (job.0)();
            }
        }
        None => job(),
    }
    coming
}

pub(crate) fn line(frames: f64) {
    let waited = WAITED.swap(0, Ordering::Relaxed) as f64 / 1e3 / frames.max(1.0);
    if waited > 0.0 {
        crate::xlog_info!("D3D12.X: XCL_HEAP_PASS_ASIDE per frame: {waited:.2} ms lists waited at close");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_is_waited_and_panic_lets_list_go_on() {
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let coming = begin(move || {
            let _ = wait.recv();
            Vec::new()
        });
        let _ = go.send(());
        assert!(coming.wait().is_empty());
        assert!(begin(|| panic!("aside panic")).wait().is_empty());
        assert!(begin(Vec::new).wait().is_empty());
    }
}
