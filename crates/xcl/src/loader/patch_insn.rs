//! rel32 stubs for loader patches (`write_sites`, similar to `descriptor_sites::insn`).

use std::sync::Mutex;
use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE};

pub const STUBS_LEN: usize = 64 << 10;

static STUBS: Mutex<Option<(usize, usize)>> = Mutex::new(None);

/// `jmp rel32` from `at` to `to` when the displacement fits in 32 bits.
pub fn jump(at: usize, to: usize) -> Option<[u8; 5]> {
    let rel = i32::try_from(to as i64 - (at as i64 + 5)).ok()?;
    let [a, b, c, d] = rel.to_le_bytes();
    Some([0xe9, a, b, c, d])
}

/// Allocate executable memory within rel32 reach of `at`.
pub fn near(at: usize) -> Option<usize> {
    let at = at & !0xffff;
    (0x10_0000..0x7000_0000)
        .step_by(0x10_0000)
        .flat_map(|d| [at.checked_sub(d), at.checked_add(d)])
        .flatten()
        .find_map(|place| {
            let got =
                unsafe { VirtualAlloc(place as *const _, STUBS_LEN, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) }
                    as usize;
            (got != 0).then_some(got)
        })
}

pub fn stub_room(at: usize) -> Option<(usize, usize)> {
    let mut stubs = STUBS.lock().unwrap_or_else(|e| e.into_inner());
    if stubs.is_none() {
        *stubs = near(at).map(|place| (place, 0));
    }
    *stubs
}

pub fn bump_stub_used(used: &mut usize, n: usize) -> bool {
    let next = used.saturating_add(n.next_multiple_of(16));
    if next > STUBS_LEN {
        return false;
    }
    *used = next;
    true
}