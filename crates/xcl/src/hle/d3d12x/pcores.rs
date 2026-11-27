//! Hybrid CPU P-core pinning (`XCL_FAST_CORES`).
//!
//! On hybrid CPUs (such as Intel 12th/13th/14th-gen processors with both Performance and Efficiency
//! cores), Windows can schedule latency-sensitive render/submit threads onto slower E-cores,
//! introducing frame-time spikes and delaying GPU submissions. This module queries
//! `GetSystemCpuSetInformation` dynamically from `kernel32.dll`, selects the Group 0 CPU set IDs
//! belonging to the highest `EfficiencyClass` when at least two distinct efficiency classes exist
//! (automatically a no-op on homogeneous CPUs such as AMD Ryzen), and pins calling threads via
//! `SetThreadSelectedCpuSets`.
//!
//! Controlled by `XCL_FAST_CORES`: `0` / `off` / `false` disables; default enabled on hybrid CPUs.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::{Once, OnceLock};

/// `CPU_SET_INFORMATION_TYPE::CpuSetInformation` in `winnt.h`.
const CPU_SET_INFORMATION_TYPE_CPU_SET: u32 = 0;
/// Minimum byte size of a valid `SYSTEM_CPU_SET_INFORMATION` header + `CpuSet` fields through
/// `EfficiencyClass` and `AllFlags` / `Reserved` (`offset 0..24`).
const MIN_CPU_SET_ENTRY_SIZE: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CpuSetEntry {
    id: u32,
    group: u16,
    efficiency_class: u8,
}

fn parse_enabled(val: Option<&str>) -> bool {
    let Some(raw) = val.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    !(raw.eq_ignore_ascii_case("0") || raw.eq_ignore_ascii_case("off") || raw.eq_ignore_ascii_case("false"))
}

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| parse_enabled(std::env::var("XCL_FAST_CORES").ok().as_deref()))
}

/// Parse a raw `GetSystemCpuSetInformation` buffer into `CpuSetEntry` records.
///
/// Layout of `SYSTEM_CPU_SET_INFORMATION` (`winnt.h`):
/// - `+0`: `DWORD Size`
/// - `+4`: `DWORD Type` (`0` = `CpuSetInformation`)
/// - `+8`: `DWORD Id`
/// - `+12`: `WORD Group`
/// - `+14..18`: `LogicalProcessorIndex`, `CoreIndex`, `LastLevelCacheIndex`, `NumaNodeIndex`
/// - `+18`: `BYTE EfficiencyClass`
fn parse_cpu_set_buffer(buf: &[u8]) -> Vec<CpuSetEntry> {
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset + MIN_CPU_SET_ENTRY_SIZE <= buf.len() {
        let size = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as usize;
        if size < MIN_CPU_SET_ENTRY_SIZE || offset + size > buf.len() {
            break;
        }
        let kind = u32::from_le_bytes(buf[offset + 4..offset + 8].try_into().unwrap());
        if kind == CPU_SET_INFORMATION_TYPE_CPU_SET {
            let id = u32::from_le_bytes(buf[offset + 8..offset + 12].try_into().unwrap());
            let group = u16::from_le_bytes(buf[offset + 12..offset + 14].try_into().unwrap());
            let efficiency_class = buf[offset + 18];
            entries.push(CpuSetEntry { id, group, efficiency_class });
        }
        offset += size;
    }
    entries
}

/// Select Group 0 CPU set IDs with the highest `EfficiencyClass`, returning an empty `Vec` unless
/// Group 0 contains at least two distinct efficiency classes (auto-no-op on homogeneous CPUs).
fn select_fast_cpu_sets(entries: &[CpuSetEntry]) -> Vec<u32> {
    let mut min_eff = u8::MAX;
    let mut max_eff = u8::MIN;
    let mut any_group0 = false;
    for e in entries {
        if e.group != 0 {
            continue;
        }
        any_group0 = true;
        min_eff = min_eff.min(e.efficiency_class);
        max_eff = max_eff.max(e.efficiency_class);
    }
    if !any_group0 || min_eff == max_eff {
        return Vec::new();
    }
    entries
        .iter()
        .filter(|e| e.group == 0 && e.efficiency_class == max_eff)
        .map(|e| e.id)
        .collect()
}

type GetSystemCpuSetInformationFn = unsafe extern "system" fn(
    information: *mut u8,
    buffer_length: u32,
    returned_length: *mut u32,
    process: *mut c_void,
    flags: u32,
) -> i32;

type SetThreadSelectedCpuSetsFn = unsafe extern "system" fn(
    thread: *mut c_void,
    cpu_set_ids: *const u32,
    cpu_set_id_count: u32,
) -> i32;

unsafe fn kernel32_proc(name: &[u8]) -> Option<*const c_void> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    let h = GetModuleHandleA(b"kernel32.dll\0".as_ptr());
    if h.is_null() {
        return None;
    }
    GetProcAddress(h, name.as_ptr()).map(|f| f as *const c_void)
}

fn query_fast_cpu_sets() -> Vec<u32> {
    if !enabled() {
        return Vec::new();
    }
    unsafe {
        let Some(p) = kernel32_proc(b"GetSystemCpuSetInformation\0") else {
            return Vec::new();
        };
        let get_info: GetSystemCpuSetInformationFn = std::mem::transmute(p);
        let mut needed: u32 = 0;
        let _ = get_info(std::ptr::null_mut(), 0, &mut needed, std::ptr::null_mut(), 0);
        if needed < MIN_CPU_SET_ENTRY_SIZE as u32 {
            return Vec::new();
        }
        let mut buf = vec![0u8; needed as usize];
        let mut returned: u32 = 0;
        if get_info(buf.as_mut_ptr(), buf.len() as u32, &mut returned, std::ptr::null_mut(), 0) == 0 {
            return Vec::new();
        }
        let len = (returned as usize).min(buf.len());
        select_fast_cpu_sets(&parse_cpu_set_buffer(&buf[..len]))
    }
}

fn fast_cpu_sets() -> &'static [u32] {
    static IDS: OnceLock<Vec<u32>> = OnceLock::new();
    IDS.get_or_init(query_fast_cpu_sets)
}

fn set_thread_cpu_sets_fn() -> Option<SetThreadSelectedCpuSetsFn> {
    static FN: OnceLock<Option<SetThreadSelectedCpuSetsFn>> = OnceLock::new();
    *FN.get_or_init(|| unsafe {
        kernel32_proc(b"SetThreadSelectedCpuSets\0").map(|p| std::mem::transmute(p))
    })
}

thread_local! {
    static THREAD_PINNED: Cell<bool> = const { Cell::new(false) };
}

/// Pin the calling thread to Group 0 P-core CPU sets on hybrid CPUs.
///
/// Idempotent and cached per thread via `thread_local!`; no-op on homogeneous CPUs or when
/// `XCL_FAST_CORES=0`.
pub(super) fn pin_current_thread_fast() {
    if THREAD_PINNED.with(|p| p.replace(true)) {
        return;
    }
    let ids = fast_cpu_sets();
    if ids.is_empty() {
        return;
    }
    let Some(set_fn) = set_thread_cpu_sets_fn() else {
        return;
    };
    let ok = unsafe {
        let thread = windows_sys::Win32::System::Threading::GetCurrentThread();
        set_fn(thread, ids.as_ptr(), ids.len() as u32) != 0
    };
    if ok {
        static LOGGED: Once = Once::new();
        LOGGED.call_once(|| {
            crate::xlog_info!(
                "D3D12.X: pinned latency-sensitive thread(s) to {} P-core logical CPUs (XCL_FAST_CORES=0 to disable)",
                ids.len()
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_parsing_enables_by_default_and_disables_on_zero_or_off() {
        assert!(parse_enabled(None));
        assert!(parse_enabled(Some("")));
        assert!(parse_enabled(Some("1")));
        assert!(parse_enabled(Some("on")));
        assert!(parse_enabled(Some("true")));
        assert!(!parse_enabled(Some("0")));
        assert!(!parse_enabled(Some(" 0 ")));
        assert!(!parse_enabled(Some("off")));
        assert!(!parse_enabled(Some("OFF")));
        assert!(!parse_enabled(Some("false")));
    }

    #[test]
    fn heterogeneous_cpu_selects_highest_efficiency_group0_only() {
        // Simulates Intel i5-13600KF (12 P-core threads @ eff 1, 8 E-core threads @ eff 0)
        // plus a Group 1 entry with higher eff to verify Group 0 filtering.
        let mut entries = Vec::new();
        for id in 0x100u32..0x10C {
            entries.push(CpuSetEntry { id, group: 0, efficiency_class: 1 });
        }
        for id in 0x10Cu32..0x114 {
            entries.push(CpuSetEntry { id, group: 0, efficiency_class: 0 });
        }
        entries.push(CpuSetEntry { id: 0x200, group: 1, efficiency_class: 2 });

        let selected = select_fast_cpu_sets(&entries);
        let expected: Vec<u32> = (0x100u32..0x10C).collect();
        assert_eq!(selected, expected);
    }

    #[test]
    fn homogeneous_cpu_is_noop() {
        // Homogeneous Ryzen: all logical CPUs report the same EfficiencyClass (0).
        let ryzen: Vec<CpuSetEntry> = (0..16)
            .map(|i| CpuSetEntry { id: 256 + i, group: 0, efficiency_class: 0 })
            .collect();
        assert!(select_fast_cpu_sets(&ryzen).is_empty());

        // Homogeneous all-1 efficiency class is also a no-op.
        let uniform_one: Vec<CpuSetEntry> = (0..8)
            .map(|i| CpuSetEntry { id: 256 + i, group: 0, efficiency_class: 1 })
            .collect();
        assert!(select_fast_cpu_sets(&uniform_one).is_empty());

        // Empty list is a no-op.
        assert!(select_fast_cpu_sets(&[]).is_empty());
    }

    #[test]
    fn raw_buffer_parser_handles_variable_record_sizes_and_other_types() {
        let mut buf = Vec::new();
        let push_rec = |buf: &mut Vec<u8>, size: usize, kind: u32, id: u32, group: u16, eff: u8| {
            let start = buf.len();
            buf.resize(start + size, 0);
            buf[start..start + 4].copy_from_slice(&(size as u32).to_le_bytes());
            buf[start + 4..start + 8].copy_from_slice(&kind.to_le_bytes());
            buf[start + 8..start + 12].copy_from_slice(&id.to_le_bytes());
            buf[start + 12..start + 14].copy_from_slice(&group.to_le_bytes());
            buf[start + 18] = eff;
        };
        push_rec(&mut buf, 32, CPU_SET_INFORMATION_TYPE_CPU_SET, 0x100, 0, 1);
        push_rec(&mut buf, 40, 99, 0x999, 0, 5); // non-CpuSetInformation record skipped
        push_rec(&mut buf, 32, CPU_SET_INFORMATION_TYPE_CPU_SET, 0x101, 0, 0);
        // Trailing truncated bytes must stop cleanly without panic.
        buf.extend_from_slice(&[0u8; 8]);

        let parsed = parse_cpu_set_buffer(&buf);
        assert_eq!(
            parsed,
            vec![
                CpuSetEntry { id: 0x100, group: 0, efficiency_class: 1 },
                CpuSetEntry { id: 0x101, group: 0, efficiency_class: 0 },
            ]
        );
        assert_eq!(select_fast_cpu_sets(&parsed), vec![0x100]);
    }
}
