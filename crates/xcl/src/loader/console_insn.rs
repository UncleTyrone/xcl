//! Zen 2 instructions the title uses (`monitorx`, `mwaitx`, `clzero` loops): patch on first
//! fault so later passes do not trip the vectored handler (defaults on; `XCL_PATCH_WAITS=0`,
//! `XCL_PATCH_CLZERO=0` to emulate every time).

use std::sync::Mutex;

use crate::insn::{self, ConsoleOnly};
use super::patch_insn::{self, STUBS_LEN};

use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

static PATCHING: Mutex<()> = Mutex::new(());
static STUBS: Mutex<Option<(usize, usize)>> = Mutex::new(None);

/// Default on; `XCL_PATCH_WAITS=0` emulates each fault.
pub fn patches_waits() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_PATCH_WAITS").is_none_or(|v| v != "0"))
}

/// Default on; `XCL_PATCH_CLZERO=0` emulates each fault.
pub fn patches_clzero() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_PATCH_CLZERO").is_none_or(|v| v != "0"))
}

pub fn wait_patch(what: ConsoleOnly) -> Option<[u8; 3]> {
    match what {
        ConsoleOnly::Monitorx => Some([0x0f, 0x1f, 0x00]),
        ConsoleOnly::Mwaitx => Some([0xf3, 0x90, 0x90]),
        _ => None,
    }
}

fn clzero_stub(at: usize, stub: usize, cmp: [u8; 3]) -> Option<Vec<u8>> {
    if !matches!(cmp[0], 0x48 | 0x4c) || cmp[1] != 0x39 || cmp[2] & 0xc7 != 0xc0 || (cmp[0] == 0x48 && cmp[2] == 0xc0) {
        return None;
    }
    let mut code = vec![
        0x51,
        0x48, 0x89, 0xc1,
        0x48, 0x83, 0xe1, 0xc0,
    ];
    for k in 0..8 {
        code.extend([0x48, 0xc7, 0x41, k * 8, 0, 0, 0, 0]);
    }
    code.push(0x59);
    code.extend([0x48, 0x83, 0xc0, 0x40]);
    code.extend(cmp);
    let back = -(code.len() as i64 + 2);
    code.extend([0x72, i8::try_from(back).ok()? as u8]);
    let end = stub + code.len();
    code.extend(patch_insn::jump(end, at + 12)?);
    Some(code)
}

fn with_patch(word: u128, at: usize, patch: &[u8]) -> Option<u128> {
    let offset = at & 15;
    if offset + patch.len() > 16 {
        return None;
    }
    let mut bytes = word.to_le_bytes();
    bytes[offset..offset + patch.len()].copy_from_slice(patch);
    Some(u128::from_le_bytes(bytes))
}

/// Single 16-byte aligned store of `patch` over `was` at `at` (requires `cmpxchg16b`).
///
/// # Safety
/// `at` lies in mapped title code that stays valid.
pub unsafe fn patch<const N: usize>(at: usize, was: [u8; N], patch: [u8; N]) -> bool {
    use std::sync::atomic::Ordering::SeqCst;
    if (at & 15) + N > 16 || !std::is_x86_feature_detected!("cmpxchg16b") {
        return false;
    }
    let _one = PATCHING.lock().unwrap_or_else(|e| e.into_inner());
    let word = (at & !15) as *mut u128;
    let here = (at & 15)..(at & 15) + N;
    let mut old = 0u32;
    if VirtualProtect(word.cast(), 16, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
        return false;
    }
    let mut now = std::arch::x86_64::cmpxchg16b(word, 0, 0, SeqCst, SeqCst);
    let done = loop {
        let bytes = &now.to_le_bytes()[here.clone()];
        if bytes == patch {
            break true;
        }
        let Some(new) = (bytes == was).then(|| with_patch(now, at, &patch)).flatten() else {
            break false;
        };
        let there = std::arch::x86_64::cmpxchg16b(word, now, new, SeqCst, SeqCst);
        if there == now {
            break true;
        }
        now = there;
    };
    VirtualProtect(word.cast(), 16, old, &mut old);
    FlushInstructionCache(GetCurrentProcess(), word.cast(), 16);
    done
}

/// Replace a 12-byte `clzero` loop at `at` with a jump to plain stores in a nearby stub page.
///
/// # Safety
/// `code` is the 12 faulting bytes at `at` in mapped title code.
pub unsafe fn patch_clzero(at: usize, code: &[u8]) -> bool {
    let &[a, b, c, d, e, _, _, x, y, z, 0x72, 0xf4] = code else {
        return false;
    };
    let mut stubs = STUBS.lock().unwrap_or_else(|e| e.into_inner());
    if stubs.is_none() {
        *stubs = patch_insn::near(at).map(|place| (place, 0));
    }
    let Some((place, used)) = stubs.as_mut() else {
        return false;
    };
    let stub = *place + *used;
    let (Some(body), Some(jump)) = (clzero_stub(at, stub, [x, y, z]), patch_insn::jump(at, stub)) else {
        return false;
    };
    if *used + body.len() > STUBS_LEN || (at & 15) + 5 > 16 {
        return false;
    }
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub as *mut u8, body.len());
    *used += body.len().next_multiple_of(16);
    patch(at, [a, b, c, d, e], jump)
}

/// Apply wait/clzero loop patches on executable regions before entry (optional speed-up).
///
/// # Safety
/// `base`/`size` span the mapped main image; section VAs are RVAs.
pub unsafe fn warm_text(base: usize, size: usize, text_vas: &[(u32, u32)]) {
    if !patches_waits() && !patches_clzero() {
        return;
    }
    let mut waits = 0usize;
    let mut clzero = 0usize;
    for &(va, raw) in text_vas {
        let start = base + va as usize;
        let len = raw as usize;
        if len == 0 || start.saturating_add(len) > base + size {
            continue;
        }
        let mut i = 0usize;
        while i + 3 <= len {
            let rip = start + i;
            let slice = std::slice::from_raw_parts(rip as *const u8, (len - i).min(16));
            let Some((what, insn_len)) = insn::decode(slice) else {
                i += 1;
                continue;
            };
            match what {
                ConsoleOnly::Monitorx | ConsoleOnly::Mwaitx if patches_waits() && insn_len == 3 => {
                    if let Some(new) = wait_patch(what) {
                        let was = [slice[0], slice[1], slice[2]];
                        if patch(rip, was, new) {
                            waits += 1;
                        }
                    }
                }
                ConsoleOnly::ClzeroLoop { .. } if patches_clzero() && insn_len == 12 && i + 12 <= len => {
                    if patch_clzero(rip, &slice[..12]) {
                        clzero += 1;
                    }
                    i += 12;
                    continue;
                }
                _ => {}
            }
            i += 1.max(insn_len.saturating_sub(1));
        }
    }
    if waits != 0 || clzero != 0 {
        crate::xlog_info!(
            "console_insn: warmed {waits} wait site(s), {clzero} clzero loop(s) (XCL_PATCH_WAITS / XCL_PATCH_CLZERO)"
        );
    }
}