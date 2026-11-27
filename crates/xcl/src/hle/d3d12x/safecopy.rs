//! Fault-tolerant copies between card views and title memory (`XCL_SAFE_COPY`, default on).
//!
//! Title pages may unmap or flip protection while GPU readback merges; a vectored handler turns
//! access faults inside the guarded `rep movsb` into a soft failure instead of killing the process.
//! Merge/put paths write only dwords that actually change so neighbouring title threads keep their
//! stores.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".globl xcl_safe_copy",
    ".globl xcl_safe_copy_at",
    ".globl xcl_safe_copy_after",
    ".globl xcl_safe_copy_failed",
    "xcl_safe_copy:",
    "    mov r9, rsi",
    "    mov r10, rdi",
    "    mov rdi, rcx",
    "    mov rsi, rdx",
    "    mov rcx, r8",
    "xcl_safe_copy_at:",
    "    rep movsb",
    "xcl_safe_copy_after:",
    "    mov rsi, r9",
    "    mov rdi, r10",
    "    mov eax, 1",
    "    ret",
    "xcl_safe_copy_failed:",
    "    mov rsi, r9",
    "    mov rdi, r10",
    "    xor eax, eax",
    "    ret",
);

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn xcl_safe_copy(dst: *mut u8, src: *const u8, len: usize) -> u32;
    static xcl_safe_copy_at: u8;
    static xcl_safe_copy_after: u8;
    static xcl_safe_copy_failed: u8;
}

/// Result of [`merge`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Merged {
    /// 32-byte blocks where card bytes differed from the held snapshot.
    pub(crate) new: u64,
    /// Dwords written into title memory.
    pub(crate) dwords: u64,
    /// False when a title page could not be read or written.
    pub(crate) there: bool,
}

static STATS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
const S_COPIES: usize = 0;
const S_MERGES: usize = 1;
const S_FAULTS: usize = 2;

/// `XCL_SAFE_COPY=0` forces off; otherwise on (including when unset).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = cfg!(target_arch = "x86_64") && std::env::var_os("XCL_SAFE_COPY").is_none_or(|v| v != "0");
        if on {
            install();
            crate::xlog_info!("D3D12.X: title↔card merges use fault-safe copies (XCL_SAFE_COPY)");
        }
        on
    })
}

#[cfg(target_arch = "x86_64")]
fn install() {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, EXCEPTION_GUARD_PAGE, EXCEPTION_IN_PAGE_ERROR};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };
    unsafe extern "system" fn on_fault(info: *mut EXCEPTION_POINTERS) -> i32 {
        let (r, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
        let rip = context.Rip as usize;
        let Some(exit) = failing_exit(rip) else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let code = r.ExceptionCode;
        if code == EXCEPTION_ACCESS_VIOLATION || code == EXCEPTION_IN_PAGE_ERROR || code == EXCEPTION_GUARD_PAGE {
            STATS[S_FAULTS].fetch_add(1, Relaxed);
            context.Rip = exit as u64;
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        EXCEPTION_CONTINUE_SEARCH
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(on_fault));
    });
}

#[cfg(not(target_arch = "x86_64"))]
fn install() {}

/// Where a fault at instruction `at` should resume (soft-fail the copy), if it is ours.
pub(crate) fn failing_exit(at: usize) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        let from = (&raw const xcl_safe_copy_at) as usize;
        let to = (&raw const xcl_safe_copy_after) as usize;
        if (from..to).contains(&at) {
            return Some((&raw const xcl_safe_copy_failed) as usize);
        }
    }
    let _ = at;
    None
}

/// Copy `len` bytes. False if a page could not be read or written (some bytes may still have moved).
///
/// # Safety
/// Ranges must be live for `len` bytes; destination must not alias a Rust reference.
pub(crate) unsafe fn copy(dst: *mut u8, src: *const u8, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    if !on() {
        std::ptr::copy_nonoverlapping(src, dst, len);
        return true;
    }
    STATS[S_COPIES].fetch_add(1, Relaxed);
    #[cfg(target_arch = "x86_64")]
    {
        install();
        return unsafe { xcl_safe_copy(dst, src, len) != 0 };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        std::ptr::copy_nonoverlapping(src, dst, len);
        true
    }
}

/// Store `now` at `dst` only where it differs from `was`, as whole aligned dwords (and byte tails).
///
/// # Safety
/// As [`copy`].
pub(crate) unsafe fn put_changed(dst: *mut u8, was: &[u8], now: &[u8]) -> bool {
    assert_eq!(was.len(), now.len());
    let n = now.len();
    if n == 0 {
        return true;
    }
    let mut all = true;
    let mut i = 0usize;
    while i < n {
        let addr = dst as usize + i;
        let room = (4 - (addr % 4)).min(n - i);
        if was[i..i + room] != now[i..i + room] {
            all &= unsafe { copy(dst.add(i), now[i..].as_ptr(), room) };
        }
        i += room;
    }
    all
}

/// Merge card bytes `now` into title `title`, updating `held` where they differ.
/// When a title page is missing and `take` is set, `held` still takes `now`.
///
/// # Safety
/// `now`/`held` are host buffers of `len`; `title` is title memory for the same span.
pub(crate) unsafe fn merge(title: *mut u8, now: *const u8, held: *mut u8, len: usize, take: bool) -> Merged {
    let mut out = Merged { there: true, ..Merged::default() };
    if len == 0 {
        return out;
    }
    STATS[S_MERGES].fetch_add(1, Relaxed);
    let mut at = 0usize;
    while at + 4 <= len {
        let card = unsafe { std::slice::from_raw_parts(now.add(at), 4) };
        let snap = unsafe { std::slice::from_raw_parts(held.add(at), 4) };
        if card != snap {
            out.new += 1;
            let mut cur = [0u8; 4];
            let ok_read = unsafe { copy(cur.as_mut_ptr(), title.add(at), 4) };
            out.there &= ok_read;
            if ok_read && cur != *card {
                let ok_write = unsafe { copy(title.add(at), card.as_ptr(), 4) };
                out.there &= ok_write;
                if ok_write {
                    out.dwords += 1;
                }
            }
            if ok_read || take {
                unsafe { std::ptr::copy_nonoverlapping(card.as_ptr(), held.add(at), 4) };
            }
        }
        at += 4;
    }
    while at < len {
        let byte = unsafe { now.add(at).read() };
        let snap = unsafe { held.add(at).read() };
        if byte != snap {
            out.new += 1;
            let mut cur = 0u8;
            let ok_read = unsafe { copy(&mut cur, title.add(at), 1) };
            out.there &= ok_read;
            if ok_read && cur != byte {
                let ok_write = unsafe { copy(title.add(at), &byte, 1) };
                out.there &= ok_write;
                if ok_write {
                    out.dwords += 1;
                }
            }
            if ok_read || take {
                unsafe { held.add(at).write(byte) };
            }
        }
        at += 1;
    }
    out
}

/// Soft stats for the activity line.
pub(crate) fn log_stats(frames: f64) {
    if !on() || frames <= 0.0 {
        return;
    }
    let merges = STATS[S_MERGES].swap(0, Relaxed) as f64 / frames;
    if merges == 0.0 {
        return;
    }
    let copies = STATS[S_COPIES].swap(0, Relaxed) as f64 / frames;
    let faults = STATS[S_FAULTS].swap(0, Relaxed) as f64 / frames;
    crate::xlog_info!(
        "D3D12.X: safecopy per frame: {:.0} merges, {:.0} copies{}",
        merges,
        copies,
        if faults > 0.0 {
            format!(", {:.0} soft faults", faults)
        } else {
            String::new()
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_writes_only_changed_dwords() {
        let mut title = [0x11u8; 16];
        let now = [0x11u8, 0x11, 0x11, 0x11, 0x22, 0x22, 0x22, 0x22, 0x11, 0x11, 0x11, 0x11, 0x33, 0x33, 0x33, 0x33];
        let mut held = [0x11u8; 16];
        let m = unsafe { merge(title.as_mut_ptr(), now.as_ptr(), held.as_mut_ptr(), 16, true) };
        assert!(m.there);
        assert_eq!(m.new, 2);
        assert_eq!(m.dwords, 2);
        assert_eq!(&title[4..8], &[0x22; 4]);
        assert_eq!(&title[12..16], &[0x33; 4]);
        assert_eq!(&title[..4], &[0x11; 4]);
        assert_eq!(&held, &now);
    }

    #[test]
    fn put_changed_skips_equal_dwords() {
        let mut dst = [0xAAu8; 8];
        let was = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut now = was;
        now[4] = 9;
        assert!(unsafe { put_changed(dst.as_mut_ptr(), &was, &now) });
        assert_eq!(&dst[..4], &[0xAA; 4]);
        assert_eq!(&dst[4..], &now[4..]);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn copy_matches_plain_bytes() {
        let from: Vec<u8> = (0..4096u32).map(|i| (i * 17) as u8).collect();
        let mut to = vec![0u8; from.len()];
        assert!(unsafe { copy(to.as_mut_ptr(), from.as_ptr(), from.len()) });
        assert_eq!(to, from);
    }
}
