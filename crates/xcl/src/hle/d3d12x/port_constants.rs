//! Belobug renderer port 1 (`XCL_PORT_CONSTANTS`): title constants in place via a second view of
//! the physical-page section and `OpenExistingHeapFromAddress1`, without host copy, write-watch
//! upload, or chunk hashing on that range.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;
use windows_sys::Win32::System::Memory as mem;

use super::port;
use super::real;

const K64: u64 = 64 << 10;
const PAGE: usize = 64 << 10;
/// Title graphics memory for 32-bit GPU addresses.
const LOW: u64 = 1 << 32;
const MOST: u64 = 512 << 20;

/// Console `XALLOC_ATTRIBUTES` bit: allocate below 4 GiB.
const ATTR_BELOW_4G: u64 = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cover {
    None,
    Holes,
    Scattered,
    Run(usize, u64),
}

#[derive(Default)]
struct Views {
    /// address → (bytes, section handle, offset in section)
    views: BTreeMap<usize, (usize, usize, u64)>,
}

static VIEWS: Mutex<Views> = Mutex::new(Views { views: BTreeMap::new() });
static LOW_RESERVATIONS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
static PLACEHOLDER_PIECES: AtomicU64 = AtomicU64::new(0);

static REGIONS: Mutex<Vec<Region>> = Mutex::new(Vec::new());
static BOUNDS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
/// placements, regions, region-bytes, refuse-stretch, refuse-commit, refuse-heap, refuse-buffer, forgotten
static STATS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

struct Region {
    start: u64,
    end: u64,
    heap: ID3D12Heap,
}

/// A buffer placed over title memory in an opened heap.
pub struct Placed {
    pub buffer: ID3D12Resource,
    pub heap: ID3D12Heap,
    pub gpu: u64,
    pub bias: u64,
}

pub fn on() -> bool {
    port::constants()
}

pub fn wanted(at: u64, len: u64) -> bool {
    on() && at.checked_add(len).is_some_and(|end| end <= LOW && len > 0)
}

pub fn note_low_reservation(base: usize, bytes: usize) {
    if !on() || bytes == 0 {
        return;
    }
    let mut g = LOW_RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner());
    if g.iter().any(|&(b, _)| b == base) {
        return;
    }
    g.push((base, bytes));
    let n = g.len();
    if n <= 4 || n.is_power_of_two() {
        crate::xlog_info!(
            "renderer port 1: recorded 32-bit graphics reservation at {base:#x} ({bytes:#x} bytes, {} placeholder splits)",
            PLACEHOLDER_PIECES.load(Ordering::Relaxed)
        );
    }
}

pub fn note_placeholder_split() {
    if !on() {
        return;
    }
    let n = PLACEHOLDER_PIECES.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 5 {
        crate::xlog_info!("renderer port 1: saw five-placeholder graphics heap split (port1 setup)");
    }
}

/// After `alias` maps `[at, at + len)` of a section.
pub fn note_view(at: usize, len: usize, section: usize, offset: u64) {
    if !on() || len == 0 {
        return;
    }
    let mut v = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    v.views.insert(at, (len, section, offset));
}

pub fn views_gone(at: usize, len: usize) {
    if !on() {
        return;
    }
    forget(at, len);
    let mut v = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let end = at.saturating_add(len);
    v.views.retain(|&base, entry| base + entry.0 <= at || base >= end);
}

pub fn forget(at: usize, len: usize) {
    if !on() {
        return;
    }
    let (at, end) = (at as u64, (at as u64).saturating_add(len as u64));
    if BOUNDS[1].load(Ordering::Relaxed) <= at || BOUNDS[0].load(Ordering::Relaxed) >= end {
        return;
    }
    let before = REGIONS.lock().unwrap_or_else(|e| e.into_inner()).len();
    REGIONS.lock().unwrap_or_else(|e| e.into_inner()).retain(|r| r.end <= at || r.start >= end);
    let after = REGIONS.lock().unwrap_or_else(|e| e.into_inner()).len();
    if before > after {
        STATS[7].fetch_add((before - after) as u64, Ordering::Relaxed);
        refresh_bounds();
    }
}

fn cover(at: usize, len: usize) -> Cover {
    let v = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let end = at + len.max(PAGE);
    let Some((&base, &(bytes, section, offset))) = v.views.range(..=at).next_back().filter(|(b, view)| *b + view.0 > at) else {
        return Cover::None;
    };
    let first = (section, offset + (at - base) as u64);
    let delta = at as i128 - first.1 as i128;
    let follows = |a: usize, s: usize, o: u64| s == first.0 && a as i128 - o as i128 == delta;
    let mut reached = base + bytes;
    let mut one = true;
    while reached < end {
        let Some(&(bytes, s, o)) = v.views.get(&reached) else {
            return Cover::Holes;
        };
        one &= follows(reached, s, o);
        reached += bytes;
    }
    if one { Cover::Run(first.0, first.1) } else { Cover::Scattered }
}

fn stretch(at: usize, len: usize) -> Option<(usize, usize, usize, u64)> {
    let v = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let Cover::Run(section, offset) = cover(at, len) else {
        return None;
    };
    let delta = at as i128 - offset as i128;
    let follows = |a: usize, s: usize, o: u64| s == section && a as i128 - o as i128 == delta;
    let (&first, &(n, _, _)) = v.views.range(..=at).next_back()?;
    let mut start = first;
    for (&a, &(bytes, s, o)) in v.views.range(..first).rev() {
        if a + bytes != start || !follows(a, s, o) {
            break;
        }
        start = a;
    }
    let mut stop = first + n;
    for (&a, &(bytes, s, o)) in v.views.range(first + 1..) {
        if a != stop || !follows(a, s, o) {
            break;
        }
        stop = a + bytes;
    }
    Some((start, stop - start, section, (start as i128 - delta) as u64))
}

fn committed(at: usize, len: usize) -> bool {
    let mut here = at;
    while here < at + len {
        let mut info: mem::MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { mem::VirtualQuery(here as *const _, &mut info, std::mem::size_of_val(&info)) } == 0
            || info.State != mem::MEM_COMMIT
        {
            return false;
        }
        here = (info.BaseAddress as usize).saturating_add(info.RegionSize);
    }
    true
}

fn refresh_bounds() {
    let g = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    let lo = g.iter().map(|r| r.start).min().unwrap_or(0);
    let hi = g.iter().map(|r| r.end).max().unwrap_or(0);
    BOUNDS[0].store(lo, Ordering::Relaxed);
    BOUNDS[1].store(hi, Ordering::Relaxed);
}

fn touches(at: usize, len: usize) -> bool {
    let hi = BOUNDS[1].load(Ordering::Relaxed);
    hi != 0 && (at as u64) < hi && (at as u64).saturating_add(len as u64) > BOUNDS[0].load(Ordering::Relaxed)
}

pub fn touches_page(page: usize) -> bool {
    touches(page, 4096)
}

pub fn touches_range(at: usize, len: usize) -> bool {
    touches(at, len)
}

fn open(device: &ID3D12Device, at: usize, bytes: usize) -> Result<ID3D12Heap, String> {
    unsafe {
        let dev13: ID3D12Device13 = device.clone().cast().map_err(|e| format!("{e}"))?;
        dev13.OpenExistingHeapFromAddress1(at as *const std::ffi::c_void, bytes).map_err(|e| format!("{e}"))
    }
}

fn place_buffer(device: &ID3D12Device, heap: &ID3D12Heap, offset: u64, size: u64) -> Result<ID3D12Resource, String> {
    let mut buf: Option<ID3D12Resource> = None;
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size.max(1),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ..Default::default()
    };
    unsafe {
        device
            .CreatePlacedResource(heap, offset, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut buf)
            .map_err(|e| format!("CreatePlacedResource: {e}"))?;
    }
    buf.ok_or_else(|| "CreatePlacedResource".to_owned())
}

fn open_region(device: &ID3D12Device, at: u64, end: u64) -> Result<Region, (usize, String)> {
    let Some((first, bytes, section, offset)) = stretch(at as usize, (end - at) as usize) else {
        return Err((3, "not one stretch of a section".to_owned()));
    };
    let (mut first, mut bytes, mut offset) = (first as u64, bytes as u64, offset);
    if first < LOW && first + bytes > LOW {
        bytes = LOW.saturating_sub(first);
    }
    if bytes > MOST {
        offset += at - first;
        (first, bytes) = (at, end - at);
    }
    if bytes > MOST {
        return Err((3, format!("{bytes:#x} bytes over region limit")));
    }
    if first > at || first + bytes < end || offset % K64 != 0 {
        return Err((3, format!("stretch {first:#x}+{bytes:#x} does not hold range")));
    }
    let view = unsafe {
        mem::MapViewOfFile(
            section as _,
            mem::FILE_MAP_WRITE,
            (offset >> 32) as u32,
            offset as u32,
            bytes as usize,
        )
    }
    .Value as usize;
    if view == 0 {
        return Err((5, format!("MapViewOfFile failed: {}", std::io::Error::last_os_error())));
    }
    if !committed(view, bytes as usize) {
        unsafe { mem::UnmapViewOfFile(mem::MEMORY_MAPPED_VIEW_ADDRESS { Value: view as *mut _ }) };
        return Err((4, "pages not committed".to_owned()));
    }
    match open(device, view, bytes as usize) {
        Ok(heap) => {
            crate::xlog_info!(
                "renderer port 1: graphics stretch {first:#x}+{bytes:#x} opened (section {section:#x} offset {offset:#x} view {view:#x})"
            );
            Ok(Region { start: first, end: first + bytes, heap })
        }
        Err(e) => {
            unsafe { mem::UnmapViewOfFile(mem::MEMORY_MAPPED_VIEW_ADDRESS { Value: view as *mut _ }) };
            Err((5, e))
        }
    }
}

pub fn place_in(device: &ID3D12Device, start: u64, len: u64) -> Result<Placed, (usize, String)> {
    let at = start & !(K64 - 1);
    let end = (start + len.max(1)).next_multiple_of(K64);
    let mut regions = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    let i = match regions.iter().position(|r| r.start <= at && end <= r.end) {
        Some(i) => i,
        None => {
            let region = open_region(device, at, end)?;
            regions.push(region);
            let idx = regions.len() - 1;
            let r = &regions[idx];
            STATS[1].fetch_add(1, Ordering::Relaxed);
            STATS[2].fetch_add(r.end.saturating_sub(r.start), Ordering::Relaxed);
            refresh_bounds();
            idx
        }
    };
    let region = &regions[i];
    let buffer = place_buffer(device, &region.heap, at - region.start, end - at).map_err(|e| (6_usize, e))?;
    let gpu = unsafe { buffer.GetGPUVirtualAddress() } + (start - at);
    Ok(Placed { buffer, heap: region.heap.clone(), gpu, bias: start - at })
}

pub fn place_for(start: u64, len: u64) -> Option<Placed> {
    if !wanted(start, len) {
        return None;
    }
    let raw = real::device_raw();
    if raw.is_null() {
        return None;
    }
    let device = unsafe { ID3D12Device::from_raw_borrowed(&raw) }?.clone();
    match place_in(&device, start, len) {
        Ok(placed) => {
            let n = STATS[0].fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 32 || n.is_power_of_two() {
                crate::xlog_info!("renderer port 1: buffer placed at {start:#x} ({len:#x} bytes)");
            }
            Some(placed)
        }
        Err((code, why)) => {
            let stat = match code {
                3 => 3,
                4 => 4,
                5 => 5,
                6 => 6,
                _ => 3,
            };
            STATS[stat].fetch_add(1, Ordering::Relaxed);
            let n = STATS[stat].load(Ordering::Relaxed);
            if n <= 8 || n.is_power_of_two() {
                crate::xlog_warn!("renderer port 1: not in place at {start:#x} ({len:#x} bytes): {why}");
            }
            None
        }
    }
}

/// Resource pointer for a console buffer placed in the constants heap (caller registers tracking).
pub unsafe fn place_console_buffer(
    va: usize,
    bytes: usize,
    _riid: *const windows_sys::core::GUID,
    ppv: *mut *mut std::ffi::c_void,
) -> Option<crate::util::HRESULT> {
    if ppv.is_null() {
        return None;
    }
    let placed = place_for(va as u64, bytes as u64)?;
    let Placed { buffer, heap, .. } = placed;
    std::mem::forget(heap);
    *ppv = buffer.into_raw() as *mut _;
    Some(0)
}

pub fn attrs_below_4g(attrs: u64) -> bool {
    attrs & ATTR_BELOW_4G != 0
}

static LAST_LOG: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Once per second stats line (call from present).
pub fn tick_log() {
    if !on() {
        return;
    }
    let mut last = LAST_LOG.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    if last.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_secs(1)) {
        return;
    }
    *last = Some(now);
    let s: Vec<u64> = STATS.iter().map(|x| x.load(Ordering::Relaxed)).collect();
    let low = LOW_RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner()).len();
    crate::xlog_info!(
        "renderer port 1 (XCL_PORT_CONSTANTS=1): {low} low reservations, {} regions ({} MB), {} placements; \
         refused stretch {} commit {} heap {} buffer {}; {} forgotten",
        s[1],
        s[2] >> 20,
        s[0],
        s[3],
        s[4],
        s[5],
        s[6],
        s[7],
    );
}
