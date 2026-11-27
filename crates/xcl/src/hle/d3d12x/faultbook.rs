//! Book of title write-fault / land / watch events (`XCL_FAULT_BOOK=<file>`).
//!
//! Samples for 40 frames from each of the times [`super::diag_env::fault_book_at`] gives
//! (seconds after start; default 150,210,270). Written as `<file>.<n>.bin`, records of 40 bytes:
//! QPC ticks, two words, a third word, thread id, what-code.
//!
//! CGBJ `victory-gpu` faultbook is study-only (GPL-3); this is a GPL-2 reimplementation wired to
//! XCL land/watch/present.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// What a record is of (stable numeric codes for offline tools).
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum What {
    Frame = 0,
    Fault = 1,
    Taken = 2,
    Known = 3,
    Asked = 4,
    Ours = 5,
    Alias = 6,
    Open = 7,
    Site = 9,
    Stored = 10,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Record {
    at: i64,
    a: u64,
    b: u64,
    c: u64,
    thread: u32,
    what: u32,
}

static RECORDING: AtomicBool = AtomicBool::new(false);
static BOOK: Mutex<Vec<Record>> = Mutex::new(Vec::new());

fn ticks() -> i64 {
    let mut t = 0i64;
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut t) };
    t
}

/// Whether records are kept now (inside an active 40-frame window).
pub(crate) fn recording() -> bool {
    RECORDING.load(Relaxed)
}

pub(crate) fn note(what: What, a: usize, b: usize, c: u64) {
    if !recording() {
        return;
    }
    let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let record = Record {
        at: ticks(),
        a: a as u64,
        b: b as u64,
        c,
        thread,
        what: what as u32,
    };
    BOOK.lock().unwrap_or_else(|e| e.into_inner()).push(record);
}

/// A present/frame ended — arm or flush the book per `XCL_FAULT_BOOK` / `_AT`.
pub(crate) fn on_frame(frame: u64) {
    const FRAMES: u64 = 40;
    static BEGAN: OnceLock<Instant> = OnceLock::new();
    static LEFT: AtomicU64 = AtomicU64::new(0);
    static MADE: AtomicU64 = AtomicU64::new(0);

    let Some(path) = super::diag_env::fault_book() else {
        return;
    };
    let times = super::diag_env::fault_book_at();
    let began = *BEGAN.get_or_init(Instant::now);

    note(What::Frame, frame as usize, 0, 0);

    if recording() {
        if LEFT.fetch_sub(1, Relaxed) > 1 {
            return;
        }
        RECORDING.store(false, Relaxed);
        let records = std::mem::take(&mut *BOOK.lock().unwrap_or_else(|e| e.into_inner()));
        let n = MADE.fetch_add(1, Relaxed);
        let file = format!("{}.{n}.bin", path.display());
        std::thread::spawn(move || {
            let bytes = unsafe {
                std::slice::from_raw_parts(records.as_ptr().cast::<u8>(), std::mem::size_of_val(records.as_slice()))
            };
            match std::fs::write(&file, bytes) {
                Ok(()) => crate::xlog_info!("D3D12.X: fault book written ({file}, {} records)", records.len()),
                Err(e) => crate::xlog_warn!("D3D12.X: fault book not written ({file}): {e}"),
            }
        });
    } else if times.get(MADE.load(Relaxed) as usize).is_some_and(|&t| began.elapsed().as_secs() >= t) {
        LEFT.store(FRAMES, Relaxed);
        RECORDING.store(true, Relaxed);
        crate::xlog_info!(
            "D3D12.X: fault book sampling for {FRAMES} frames (XCL_FAULT_BOOK, window #{})",
            MADE.load(Relaxed)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_layout_is_40_bytes() {
        assert_eq!(std::mem::size_of::<Record>(), 40);
    }

    #[test]
    fn note_is_noop_when_not_recording() {
        RECORDING.store(false, Relaxed);
        note(What::Fault, 1, 2, 3);
        assert!(BOOK.lock().unwrap().is_empty());
    }
}
