//! Submit-thread coordination: refresh lock, optional async worker, `pause_refresh` during GPU wait.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::{Mutex, OnceLock};

/// Title-side refresh lock (texture look / upload prepare). Released while submit waits on the GPU.
static REFRESH: Mutex<()> = Mutex::new(());

thread_local! {
    static HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static GUARD: std::cell::RefCell<Option<ManuallyDrop<std::sync::MutexGuard<'static, ()>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Begin holding the refresh lock; stored in thread-local for [`pause_refresh`].
pub fn begin_refresh() {
    if HELD.get() {
        return;
    }
    let g = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
    HELD.set(true);
    GUARD.with(|c| *c.borrow_mut() = Some(ManuallyDrop::new(g)));
}

pub fn end_refresh() {
    if !HELD.get() {
        return;
    }
    GUARD.with(|c| {
        if let Some(g) = c.borrow_mut().take() {
            drop(ManuallyDrop::into_inner(g));
        }
    });
    HELD.set(false);
}

/// Whether this thread holds the submit refresh lock.
pub fn refresh_held_on_this_thread() -> bool {
    HELD.get()
}

/// Drop the refresh lock for the duration of `f` (GPU or CPU wait during submit).
pub fn pause_refresh<R>(f: impl FnOnce() -> R) -> R {
    if !HELD.get() {
        return f();
    }
    let taken = GUARD.with(|c| c.borrow_mut().take());
    if let Some(mut g) = taken {
        let guard = unsafe { ManuallyDrop::take(&mut g) };
        drop(guard);
        let out = f();
        let again = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
        GUARD.with(|c| *c.borrow_mut() = Some(ManuallyDrop::new(again)));
        return out;
    }
    f()
}

static ASYNC_LOGGED: OnceLock<()> = OnceLock::new();

fn log_async_once() {
    ASYNC_LOGGED.get_or_init(|| {
        crate::xlog_info!("D3D12.X: async submit (XCL_ASYNC_SUBMIT=1; translate/join on caller, GPU on worker)");
    });
}

/// Run mapped PC lists on a helper thread after translation joined on the caller (lists are pinned
/// until this returns).
pub unsafe fn run_async_mapped(queue: *mut c_void, mapped: Vec<*mut c_void>) -> bool {
    if !super::port::async_submit() || mapped.is_empty() {
        return false;
    }
    log_async_once();
    end_refresh();
    let queue_addr = queue as usize;
    let mapped_addrs: Vec<usize> = mapped.iter().map(|&p| p as usize).collect();
    let ran = std::thread::scope(|scope| {
        let handle = scope.spawn(move || {
            begin_refresh();
            let q = queue_addr as *mut c_void;
            let raw: Vec<*mut c_void> = mapped_addrs.iter().map(|&p| p as *mut c_void).collect();
            // SAFETY: mapped PC lists joined on the title thread; valid until this returns.
            unsafe { super::real::execute_mapped_lists(q, &raw) };
            end_refresh();
        });
        handle.join().is_ok()
    });
    begin_refresh();
    ran
}

/// Wrap driver `ExecuteCommandLists` so refresh lock is not held across the GPU wait.
pub fn driver_execute<R>(f: impl FnOnce() -> R) -> R {
    pause_refresh(f)
}

pub fn present_may_queue() -> bool {
    super::late_wait::present_ahead() && super::port::async_submit()
}