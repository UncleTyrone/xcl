//! In-place title-memory heaps (GPL-2 rewrite of CGBJ `victory-gpu` inplace place core).
//!
//! When `XCL_INPLACE` is set, small title buffers that are private committed RW memory can be
//! opened via `OpenExistingHeapFromAddress` / `CreatePlacedResource` so shaders see the title's
//! own bytes (no host copy). Study of CGBJ only — not a paste. Big card-sided copies stay on the
//! twin path (`card_shadow`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows_sys::Win32::System::Memory::{
    VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, MEM_PRIVATE, PAGE_READWRITE,
};

use super::misc_env;
use super::real;

const K64: u64 = 64 << 10;

/// Placed buffer over title memory (heap + GPU VA + bias to title start).
pub(crate) struct Placed {
    pub(crate) buffer: ID3D12Resource,
    pub(crate) heap: ID3D12Heap,
    pub(crate) gpu: u64,
    pub(crate) bias: u64,
}

/// 0 off; 1 windows; 2 title buffers too (`XCL_INPLACE` / probe).
pub(crate) fn level() -> u8 {
    static L: OnceLock<u8> = OnceLock::new();
    *L.get_or_init(|| {
        if !misc_env::inplace() {
            return 0;
        }
        if misc_env::inplace_probe() {
            2
        } else {
            2
        }
    })
}

fn most_bytes() -> u64 {
    misc_env::inplace_mb()
        .unwrap_or(512)
        .saturating_mul(1 << 20)
        .max(1 << 20)
}

/// [0] windows, [1] window bytes, [2] title buffers, [3] title bytes,
/// [4] not-private, [5] host refuse, [6] over budget, [7] let-go.
static STATS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Whether `[at, at+len)` is private committed read-write covering the whole span.
pub(crate) fn private_whole(at: usize, len: usize) -> bool {
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let n = unsafe { VirtualQuery(at as *const c_void, &mut info, std::mem::size_of_val(&info)) };
    n != 0
        && info.State == MEM_COMMIT
        && info.Type == MEM_PRIVATE
        && info.Protect == PAGE_READWRITE
        && (info.BaseAddress as usize).saturating_add(info.RegionSize) >= at.saturating_add(len)
}

fn buffer_desc(size: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size.max(1),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ..Default::default()
    }
}

fn open_heap(device: &ID3D12Device, at: usize, bytes: usize) -> windows::core::Result<ID3D12Heap> {
    unsafe {
        if let Ok(dev13) = device.cast::<ID3D12Device13>() {
            return dev13.OpenExistingHeapFromAddress1(at as *const c_void, bytes);
        }
        device
            .cast::<ID3D12Device3>()?
            .OpenExistingHeapFromAddress(at as *const c_void)
    }
}

fn place_buffer(
    device: &ID3D12Device,
    heap: &ID3D12Heap,
    offset: u64,
    size: u64,
) -> windows::core::Result<ID3D12Resource> {
    let mut placed: Option<ID3D12Resource> = None;
    unsafe {
        device.CreatePlacedResource(
            heap,
            offset,
            &buffer_desc(size),
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut placed,
        )?;
    }
    placed.ok_or_else(windows::core::Error::empty)
}

/// Open 64 KiB pages holding `[start, start+len)` as a heap and place one buffer.
pub(crate) fn place_part(device: &ID3D12Device, start: u64, len: u64) -> Result<Placed, Option<windows::core::Error>> {
    let at = start & !(K64 - 1);
    let bytes = (start + len.max(1)).next_multiple_of(K64) - at;
    if !private_whole(at as usize, bytes as usize) {
        return Err(None);
    }
    let heap = open_heap(device, at as usize, bytes as usize).map_err(Some)?;
    let buffer = place_buffer(device, &heap, 0, bytes).map_err(Some)?;
    let gpu = unsafe { buffer.GetGPUVirtualAddress() } + (start - at);
    Ok(Placed {
        buffer,
        heap,
        gpu,
        bias: start - at,
    })
}

/// Title memory in place when [`level`] asks for it; `None` → caller makes a host copy.
pub(crate) fn place_for(start: u64, len: u64, window: bool) -> Option<Placed> {
    if level() < if window { 1 } else { 2 } {
        return None;
    }
    let live = STATS[1].load(Ordering::Relaxed) + STATS[3].load(Ordering::Relaxed);
    if live.saturating_add(len) > most_bytes() {
        STATS[6].fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let raw = real::device_raw();
    if raw.is_null() {
        return None;
    }
    let device = unsafe { ID3D12Device::from_raw_borrowed(&raw) }?.clone();
    match place_part(&device, start, len) {
        Ok(placed) => {
            let at = if window { 0 } else { 2 };
            STATS[at].fetch_add(1, Ordering::Relaxed);
            STATS[at + 1].fetch_add(len, Ordering::Relaxed);
            let n = STATS[at].load(Ordering::Relaxed);
            if n <= 8 || n.is_power_of_two() {
                crate::xlog_info!(
                    "D3D12.X: title memory in place at {start:#x}+{len:#x} (XCL_INPLACE{})",
                    if window { " window" } else { "" }
                );
            }
            Some(placed)
        }
        Err(None) => {
            STATS[4].fetch_add(1, Ordering::Relaxed);
            None
        }
        Err(Some(e)) => {
            let n = STATS[5].fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 8 || n.is_power_of_two() {
                crate::xlog_warn!(
                    "D3D12.X: inplace refuse at {start:#x}+{len:#x}: {e}"
                );
            }
            None
        }
    }
}

/// Count something placed elsewhere (port constants) against the same budget.
pub(crate) fn counted(window: bool, bytes: u64) {
    let at = if window { 0 } else { 2 };
    STATS[at].fetch_add(1, Ordering::Relaxed);
    STATS[at + 1].fetch_add(bytes, Ordering::Relaxed);
}

pub(crate) fn gone(window: bool, bytes: u64) {
    let at = if window { 0 } else { 2 };
    STATS[at].fetch_sub(1, Ordering::Relaxed);
    STATS[at + 1].fetch_sub(bytes, Ordering::Relaxed);
}

pub(crate) fn note_let_go(n: u64) {
    STATS[7].fetch_add(n, Ordering::Relaxed);
}
