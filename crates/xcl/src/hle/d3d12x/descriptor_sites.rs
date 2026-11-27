//! Renderer port 2 (`XCL_PORT_DESCRIPTORS=1`): byte-checked hooks on the title's
//! `vmovups` stores that copy 32-byte descriptor records into shader-visible tables
//! (game_scarlett_master.exe 1.0.2.1).

use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering::Relaxed};

mod insn {
    use std::sync::Mutex;
    use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE};

    pub const STUBS_LEN: usize = 64 << 10;

    /// `jmp rel32` from `at` to `to` when the displacement fits in 32 bits.
    pub fn jump(at: usize, to: usize) -> Option<[u8; 5]> {
        let rel = i32::try_from(to as i64 - (at as i64 + 5)).ok()?;
        let [a, b, c, d] = rel.to_le_bytes();
        Some([0xe9, a, b, c, d])
    }

    pub(crate) static STUBS: Mutex<Option<(usize, usize)>> = Mutex::new(None);

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
}

/// Cells the trampolines read ([`MARKS_LEN`] bytes): heap base, length, slot map, chunk map.
pub const MARKS_LEN: usize = 32;

/// A byte-checked patch site in the title image.
pub struct Site {
    pub window: usize,
    pub store: usize,
    pub len: usize,
    pub bytes: &'static [u8],
}

/// Six record stores in the table fillers (+0x4d9efd0 graphics, +0x4eec8e0 compute).
pub const SITES: [Site; 6] = [
    Site {
        window: 0x4d9f04f,
        store: 10,
        len: 5,
        bytes: &[
            0x81, 0xe1, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x00, 0xc5, 0xfc, 0x11, 0x04, 0x31, 0x0f, 0xb6, 0x47, 0x0b,
            0x39, 0xc5,
        ],
    },
    Site {
        window: 0x4d9f10d,
        store: 10,
        len: 5,
        bytes: &[
            0x81, 0xe1, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x00, 0xc5, 0xfc, 0x11, 0x04, 0x31, 0x0f, 0xb6, 0x47, 0x0f,
            0x41, 0x39, 0xc5,
        ],
    },
    Site {
        window: 0x4d9f19c,
        store: 12,
        len: 6,
        bytes: &[
            0x41, 0x81, 0xe1, 0xe0, 0x7f, 0x00, 0x00, 0xc4, 0xc1, 0x7c, 0x10, 0x02, 0xc4, 0xc1, 0x7c, 0x11, 0x04, 0x31, 0x44,
            0x0f, 0xb6, 0x4f, 0x0d, 0x44, 0x39, 0xc8,
        ],
    },
    Site {
        window: 0x4eec95d,
        store: 11,
        len: 6,
        bytes: &[
            0x41, 0x81, 0xe1, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x00, 0xc4, 0x81, 0x7c, 0x11, 0x04, 0x01, 0x0f, 0xb6,
            0x42, 0x0b, 0x39, 0xc5,
        ],
    },
    Site {
        window: 0x4eeca08,
        store: 10,
        len: 6,
        bytes: &[
            0x81, 0xe7, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x06, 0xc4, 0xa1, 0x7c, 0x11, 0x04, 0x07, 0x0f, 0xb6, 0x72,
            0x0d, 0x39, 0xf0,
        ],
    },
    Site {
        window: 0x4eeca82,
        store: 11,
        len: 6,
        bytes: &[
            0x41, 0x81, 0xe2, 0xe0, 0x7f, 0x00, 0x00, 0xc5, 0xfc, 0x10, 0x00, 0xc4, 0x81, 0x7c, 0x11, 0x04, 0x02, 0x0f, 0xb6,
            0x42, 0x0f, 0x41, 0x39, 0xc6,
        ],
    },
];

/// Outcome of patching one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hooked {
    Done,
    Differs,
    Failed(&'static str),
}

/// What patched stores read; layout must match the stub (`MARKS_LEN`).
#[repr(C)]
pub struct Marks {
    start: AtomicU64,
    len: AtomicU64,
    slots: AtomicPtr<u8>,
    chunks: AtomicPtr<u8>,
}

const _: () = assert!(std::mem::size_of::<Marks>() == MARKS_LEN);

pub static MARKS: Marks = Marks {
    start: AtomicU64::new(0),
    len: AtomicU64::new(0),
    slots: AtomicPtr::new(std::ptr::null_mut()),
    chunks: AtomicPtr::new(std::ptr::null_mut()),
};

static IMAGE: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

/// Address of [`MARKS`] for stubs.
pub fn marks_address() -> usize {
    std::ptr::from_ref(&MARKS) as usize
}

/// Record how many stores were patched in the mapped main image.
pub fn note_hooked(base: usize, size: usize, patched: usize) {
    IMAGE[0].store(base, Relaxed);
    IMAGE[1].store(size, Relaxed);
    IMAGE[2].store(patched, Relaxed);
}

/// Whether any store was patched in the main image.
pub fn stores_hooked() -> bool {
    IMAGE[2].load(Relaxed) > 0
}

fn lea_of(store: &[u8]) -> Option<[u8; 4]> {
    let (x, b, op_at) = match *store.first()? {
        0xc5 => (false, false, 2),
        0xc4 => {
            let p = *store.get(1)?;
            if p & 0x1f != 1 {
                return None;
            }
            (p & 0x40 == 0, p & 0x20 == 0, 3)
        }
        _ => return None,
    };
    let (op, modrm, sib) = (*store.get(op_at)?, *store.get(op_at + 1)?, *store.get(op_at + 2)?);
    if op != 0x11 || modrm != 0x04 || store.len() != op_at + 3 {
        return None;
    }
    if sib & 7 == 5 || (sib & 7 == 4 && !b) || (sib >> 3 & 7 == 4 && !x) {
        return None;
    }
    Some([0x48 | u8::from(x) << 1 | u8::from(b), 0x8d, 0x04, sib])
}

fn stub_body(store: &[u8], at: usize, to: usize, marks: usize) -> Option<Vec<u8>> {
    let lea = lea_of(store)?;
    let mut out = store.to_vec();
    out.push(0x50);
    out.extend(lea);
    out.extend([0x52, 0x51]);
    out.extend([0x48, 0xba]);
    out.extend((marks as u64).to_le_bytes());
    out.extend([0x48, 0x2b, 0x02]);
    out.extend([0x48, 0x3b, 0x42, 0x08]);
    out.extend([0x73, 0x18]);
    out.extend([0x48, 0xc1, 0xe8, 0x05]);
    out.extend([0x48, 0x8b, 0x4a, 0x10]);
    out.extend([0xc6, 0x04, 0x01, 0x01]);
    out.extend([0x48, 0xc1, 0xe8, 0x07]);
    out.extend([0x48, 0x8b, 0x4a, 0x18]);
    out.extend([0xc6, 0x04, 0x01, 0x01]);
    out.extend([0x59, 0x5a, 0x58]);
    let back = insn::jump(to + out.len(), at + store.len())?;
    out.extend(back);
    Some(out)
}

/// Patch `sites` in the image at `base` (`size` bytes). Stubs read cells at `marks`.
///
/// # Safety
/// No title code has run yet; `marks` stays valid for the image's lifetime.
pub unsafe fn apply(base: usize, size: usize, marks: usize, sites: &[Site]) -> Vec<Hooked> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut room: Option<(usize, usize)> = None;
    let mut out = Vec::new();

    for site in sites {
        let done = (|| {
            if site.window.checked_add(site.bytes.len()).is_none_or(|end| end > size)
                || site.store + site.len > site.bytes.len()
            {
                return Hooked::Failed("outside the image");
            }
            let window = base + site.window;
            let there = unsafe { std::slice::from_raw_parts(window as *const u8, site.bytes.len()) };
            if there != site.bytes {
                return Hooked::Differs;
            }
            let at = window + site.store;
            if room.is_none() {
                let mut stubs = insn::STUBS.lock().unwrap_or_else(|e| e.into_inner());
                if stubs.is_none() {
                    *stubs = insn::near(at).map(|place| (place, 0));
                }
                room = *stubs;
            }
            let Some((place, used)) = room.as_mut() else {
                return Hooked::Failed("no room for code near the title's");
            };
            let to = *place + *used;
            let store = &site.bytes[site.store..site.store + site.len];
            let (Some(body), Some(jump)) = (stub_body(store, at, to, marks), insn::jump(at, to)) else {
                return Hooked::Failed("not a store of the expected form, or out of reach");
            };
            if *used + body.len() > insn::STUBS_LEN {
                return Hooked::Failed("no room for code near the title's");
            }

            let mut old = 0;
            unsafe {
                std::ptr::copy_nonoverlapping(body.as_ptr(), to as *mut u8, body.len());
                if VirtualProtect(at as *const _, site.len, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                    return Hooked::Failed("Windows would not make it writable");
                }
                std::ptr::write_bytes(at as *mut u8, 0x90, site.len);
                std::ptr::copy_nonoverlapping(jump.as_ptr(), at as *mut u8, jump.len());
                VirtualProtect(at as *const _, site.len, old, &mut old);
                FlushInstructionCache(GetCurrentProcess(), at as *const _, site.len);
            }
            *used += body.len().next_multiple_of(16);
            if let Ok(mut stubs) = insn::STUBS.lock() {
                *stubs = room;
            }
            Hooked::Done
        })();

        let offset = site.window + site.store;
        match done {
            Hooked::Done => crate::xlog_info!(
                "renderer port 2: descriptor record store at title+{offset:#x} patched (XCL_PORT_DESCRIPTORS)"
            ),
            Hooked::Differs => {
                crate::xlog_warn!("renderer port 2: title+{offset:#x} not patched — bytes differ (another build?)")
            }
            Hooked::Failed(why) => crate::xlog_warn!("renderer port 2: title+{offset:#x} not patched ({why})"),
        }
        out.push(done);
    }
    out
}

/// After the main executable is mapped: patch stores when port 2 is on.
pub fn install_on_main(base: usize, size: usize) {
    if !super::port::descriptors() {
        return;
    }
    let marks = marks_address();
    let done = unsafe { apply(base, size, marks, &SITES) };
    let patched = done.iter().filter(|d| **d == Hooked::Done).count();
    note_hooked(base, size, patched);
    crate::xlog_info!("renderer port 2: {patched} of {} descriptor record stores patched", SITES.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_site_store_has_matching_lea() {
        let leas: Vec<[u8; 4]> = SITES
            .iter()
            .map(|s| lea_of(&s.bytes[s.store..s.store + s.len]).unwrap())
            .collect();
        assert_eq!(leas[0], [0x48, 0x8d, 0x04, 0x31]);
        assert_eq!(leas[3], [0x4b, 0x8d, 0x04, 0x01]);
        assert!(lea_of(&[0xc5, 0xfc, 0x11, 0x04, 0x24]).is_none());
    }

    #[test]
    fn unexpected_bytes_stay_untouched() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
        unsafe {
            let place = VirtualAlloc(std::ptr::null(), 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
            assert!(!place.is_null());
            let mut bytes = SITES[0].bytes.to_vec();
            bytes[3] ^= 1;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), place.add(0x100), bytes.len());
            let mine = [Site { window: 0x100, ..SITES[0] }];
            assert_eq!(apply(place as usize, 0x10000, 0, &mine), vec![Hooked::Differs]);
        }
    }
}