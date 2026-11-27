//! CGBJ Phase 13: indirect-light stand-in from screen-space radiance cells after probe resolve.
//!
//! When ray-traced bounce data is missing, `CS_ScreenProbes_TemporalResolve_*` leaves outdoor sky
//! and black indoors. After that pass (and at `CS_CacheScreenSpaceRadiance` / air volume) this
//! module raises diffuse to a frame-derived estimate: 64x36 cell means from visible radiance,
//! harmonics at 160x90, blended after resolve. R9G9B9E5 targets use packed `R32_UINT`.
//!
//! Off by default (`XCL_AMBIENT` unset / `0` / `off`). Play env: `XCL_AMBIENT=0.8`,
//! `XCL_AMBIENT_AIR=1.5`. Also accepts `VICTORY_AMBIENT` for the same scale / `constant:<level>`.
//! `XCL_AMBIENT_SETTLE` (default `250,3`), `XCL_AMBIENT_SHADE=strength,radius_m`.
//! Inactive while BVH traversal is allowed (`XCL_RT_GATE` / builder ready).

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Instant;

use windows::core::{Interface, PCSTR};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_ENABLE_STRICTNESS};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::console_list::{Layout, Param};
use super::port;
use super::{list_x, real};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Set {
    Off,
    Estimate(f32),
    Constant(f32),
}

#[derive(Clone, Copy)]
struct Settle {
    ms: f32,
    jump: f32,
    fresh: f32,
}

fn parse_set(s: &str) -> Set {
    let s = s.trim();
    let num = |t: &str| t.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0);
    match s.strip_prefix("constant:") {
        Some(rest) => num(rest).map_or(Set::Off, Set::Constant),
        None => num(s).map_or(Set::Off, Set::Estimate),
    }
}

fn raw_ambient_string() -> String {
    for key in ["XCL_AMBIENT", "VICTORY_AMBIENT", "XCL_PORT_AMBIENT"] {
        if let Ok(v) = std::env::var(key) {
            return v;
        }
    }
    port::ambient_scale().map(|n| n.to_string()).unwrap_or_default()
}

fn set() -> Set {
    static SET: OnceLock<Set> = OnceLock::new();
    *SET.get_or_init(|| {
        let raw = raw_ambient_string();
        if raw.is_empty() {
            return Set::Off;
        }
        parse_set(&raw)
    })
}

pub fn enabled() -> bool {
    set() != Set::Off
}

pub fn active() -> bool {
    enabled() && !real::bvh_traversal_allowed()
}

fn air() -> f32 {
    static V: OnceLock<f32> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("XCL_AMBIENT_AIR")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .filter(|v: &f32| v.is_finite() && *v > 0.0)
            .unwrap_or(0.0)
    })
}

fn shade() -> [f32; 2] {
    static V: OnceLock<[f32; 2]> = OnceLock::new();
    *V.get_or_init(|| {
        let mut out = [1.0, 0.6];
        if let Ok(s) = std::env::var("XCL_AMBIENT_SHADE") {
            for (d, p) in out.iter_mut().zip(s.split(',')) {
                if let Ok(v) = p.trim().parse() {
                    *d = v;
                }
            }
        }
        out
    })
}

fn settle() -> Option<Settle> {
    static S: OnceLock<Option<Settle>> = OnceLock::new();
    *S.get_or_init(|| {
        let raw = std::env::var("XCL_AMBIENT_SETTLE").unwrap_or_else(|_| "250,3".to_string());
        let mut ps = raw.split(',').map(str::trim);
        let ms = ps.next()?.parse().ok().filter(|v: &f32| v.is_finite() && *v > 0.0)?;
        let jump = ps.next().and_then(|p| p.parse().ok()).filter(|v: &f32| v.is_finite() && *v >= 0.0).unwrap_or(3.0);
        let fresh = ps.next().and_then(|p| p.parse().ok()).filter(|v: &f32| v.is_finite() && *v >= 0.0).unwrap_or(0.0);
        Some(Settle { ms, jump, fresh })
    })
}

fn frame_ms_radiance() -> f32 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    static DT: AtomicU32 = AtomicU32::new(33.0f32.to_bits());
    static ASKED: AtomicU64 = AtomicU64::new(0);
    let now = monotonic_us();
    if ASKED.swap(now, Ordering::Relaxed) == now {
        return f32::from_bits(DT.load(Ordering::Relaxed));
    }
    let prev = LAST.swap(now, Ordering::Relaxed);
    let dt = match now.saturating_sub(prev) {
        0 => 1.0,
        d if prev == 0 || d > 1_000_000 => 33.0,
        d => d as f32 * 1e-3,
    };
    DT.store(dt.to_bits(), Ordering::Relaxed);
    dt
}

fn follow_radiance() -> f32 {
    match settle() {
        Some(s) => 1.0 - (-frame_ms_radiance() / s.ms).exp(),
        None => 0.04,
    }
}

fn monotonic_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let base = EPOCH.get_or_init(Instant::now);
    base.elapsed().as_micros() as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PassRole {
    RadianceCache,
    TemporalResolve,
    InscatteringVolume,
}

fn contains_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

fn classify_cs(blob: &[u8]) -> Option<PassRole> {
    if contains_ascii_ci(blob, b"CS_ScreenProbes_TemporalResolve") {
        Some(PassRole::TemporalResolve)
    } else if contains_ascii_ci(blob, b"CS_CacheScreenSpaceRadiance") {
        Some(PassRole::RadianceCache)
    } else if contains_ascii_ci(blob, b"CS_AccumulateLocalLightInscatteringVolume") {
        Some(PassRole::InscatteringVolume)
    } else {
        None
    }
}

static PSO_ROLES: RwLock<Option<HashMap<usize, PassRole>>> = RwLock::new(None);
static HAS_CS_PSO: AtomicBool = AtomicBool::new(false);
static KEPT: AtomicBool = AtomicBool::new(false);

pub(super) fn is_ambient_compute(blob: &[u8]) -> bool {
    classify_cs(blob).is_some()
}

pub(super) fn note_compute_pso(pso: usize, cs: &[u8]) {
    if pso == 0 || !enabled() {
        return;
    }
    if let Some(role) = classify_cs(cs) {
        PSO_ROLES.write().unwrap().get_or_insert_with(HashMap::new).insert(pso, role);
        HAS_CS_PSO.store(true, Ordering::Release);
        static LOGGED: AtomicU64 = AtomicU64::new(0);
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 8 {
            crate::xlog_info!("D3D12.X: ambient: tracked {role:?} compute PSO {pso:#x}");
        }
    }
}

#[inline]
pub(super) fn has_compute_psos() -> bool {
    HAS_CS_PSO.load(Ordering::Relaxed)
}

fn role_of(pso: usize) -> Option<PassRole> {
    if pso == 0 {
        return None;
    }
    PSO_ROLES.read().unwrap().as_ref()?.get(&pso).copied()
}

/// Descriptor-heap view metadata indexed by BOTH console slot UMA address (`slot.at`, globally
/// unique across all shader-visible and CPU-only heaps) and `GLOBAL` heap descriptor index
/// (`slot.index` when hosted in `GLOBAL`) so both `CopyDescriptors*` and inline title `memcpy`
/// resolve without CPU-only heap index collisions.
#[derive(Clone, Copy, Debug)]
struct ViewEntry {
    res: usize,
    fmt: i32,
    is_uav: bool,
}

#[derive(Default)]
struct ViewSlots {
    by_addr: HashMap<usize, ViewEntry>,
    by_idx: HashMap<u32, ViewEntry>,
}

static VIEW_SLOTS: RwLock<Option<ViewSlots>> = RwLock::new(None);
static HAS_VIEW_SLOTS: AtomicBool = AtomicBool::new(false);

fn is_ambient_uav_format(fmt: i32) -> bool {
    matches!(fmt, 10 | 26 | 2 | 42)
}

fn is_depth_srv_format(fmt: i32) -> bool {
    matches!(fmt, 41 | 46 | 21 | 56)
}

fn is_depth_resource_format(fmt: i32) -> bool {
    matches!(fmt, 19 | 20 | 39 | 40 | 44 | 45 | 53 | 55)
}

/// Record a 2D texture UAV or depth SRV written at console slot address `slot_at` (and optional
/// `GLOBAL` descriptor index `global_index`, 0 when on a CPU-only / unhosted heap).
pub(super) fn note_descriptor(slot_at: usize, global_index: u32, res: usize, fmt: i32, res_fmt: i32, is_uav: bool) {
    if !enabled() || (slot_at == 0 && global_index == 0) {
        return;
    }
    if res == 0 {
        if HAS_VIEW_SLOTS.load(Ordering::Relaxed) {
            if let Some(s) = VIEW_SLOTS.write().unwrap().as_mut() {
                if slot_at != 0 {
                    s.by_addr.remove(&slot_at);
                }
                if global_index != 0 {
                    s.by_idx.remove(&global_index);
                }
            }
        }
        return;
    }
    let keep = if is_uav {
        is_ambient_uav_format(fmt)
    } else {
        is_depth_srv_format(fmt) && is_depth_resource_format(res_fmt)
    };
    let mut g = VIEW_SLOTS.write().unwrap();
    if keep {
        let s = g.get_or_insert_with(ViewSlots::default);
        let ve = ViewEntry { res, fmt, is_uav };
        if slot_at != 0 {
            s.by_addr.insert(slot_at, ve);
        }
        if global_index != 0 {
            s.by_idx.insert(global_index, ve);
        }
        HAS_VIEW_SLOTS.store(true, Ordering::Release);
    } else if let Some(s) = g.as_mut() {
        if slot_at != 0 {
            s.by_addr.remove(&slot_at);
        }
        if global_index != 0 {
            s.by_idx.remove(&global_index);
        }
    }
}

/// Evict any cached descriptor entries referencing a destroyed resource so stale raw pointers
/// are never dereferenced during dispatch lookup.
pub(super) fn forget_resource(res: usize) {
    if res == 0 || !HAS_VIEW_SLOTS.load(Ordering::Relaxed) {
        return;
    }
    if let Some(s) = VIEW_SLOTS.write().unwrap().as_mut() {
        s.by_addr.retain(|_, v| v.res != res);
        s.by_idx.retain(|_, v| v.res != res);
    }
}

/// Propagate view metadata when `CopyDescriptors*` copies from `src_at` (with pre-copy T# word 0
/// `src_global_index`) to `dst_at` (with destination `GLOBAL` index `dst_global_index`, 0 if unhosted).
pub(super) fn copy_descriptor(src_at: usize, src_global_index: u32, dst_at: usize, dst_global_index: u32) {
    if !enabled() || !HAS_VIEW_SLOTS.load(Ordering::Relaxed) || (src_at == dst_at && src_global_index == dst_global_index) {
        return;
    }
    let entry = VIEW_SLOTS.read().unwrap().as_ref().and_then(|s| {
        s.by_addr
            .get(&src_at)
            .or_else(|| (src_global_index != 0).then(|| s.by_idx.get(&src_global_index)).flatten())
            .copied()
    });
    let mut g = VIEW_SLOTS.write().unwrap();
    match entry {
        Some(v) => {
            let s = g.get_or_insert_with(ViewSlots::default);
            if dst_at != 0 {
                s.by_addr.insert(dst_at, v);
            }
            if dst_global_index != 0 {
                s.by_idx.insert(dst_global_index, v);
            }
        }
        None => {
            if let Some(s) = g.as_mut() {
                if dst_at != 0 {
                    s.by_addr.remove(&dst_at);
                }
                if dst_global_index != 0 {
                    s.by_idx.remove(&dst_global_index);
                }
            }
        }
    }
}

/// Match a screen-space resolve/reflection output UAV against dispatch grid dimensions, returning
/// a lower-is-better score when compatible (`None` if incompatible).
/// `TemporalResolve` writes diffuse screen-probe light as packed `R9G9B9E5` in `R32_UINT` (`fmt 42`);
/// restricting `TemporalResolve` to `R32_UINT` avoids accidentally overwriting a `R11G11B10_FLOAT`
/// (`fmt 26`) HDR scene color buffer bound in the same descriptor heap.
#[allow(dead_code)]
fn score_dispatch_target_for_role(role: PassRole, rd: &D3D12_RESOURCE_DESC, view_fmt: i32, groups: [u32; 3]) -> Option<u64> {
    if role == PassRole::TemporalResolve && view_fmt != 42 {
        return None;
    }
    if role == PassRole::RadianceCache && view_fmt == 42 {
        return None;
    }
    score_dispatch_target(rd, view_fmt, groups)
}

fn score_dispatch_target(rd: &D3D12_RESOURCE_DESC, view_fmt: i32, groups: [u32; 3]) -> Option<u64> {
    if rd.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || rd.DepthOrArraySize != 1
        || rd.SampleDesc.Count != 1
        || rd.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0
        || !is_ambient_uav_format(view_fmt)
    {
        return None;
    }
    let w = rd.Width as u32;
    let h = rd.Height;
    if w < 64 || h < 64 {
        return None;
    }
    // Indirect or 1D tiled dispatch (`groups[0] == 0` or `groups[1] <= 1`): match screen-space 2D
    // float UAV targets (>= 128x72), preferring larger resolution.
    if groups[0] == 0 || groups[1] <= 1 {
        if w < 128 || h < 72 {
            return None;
        }
        return Some(u64::MAX - (w as u64).saturating_mul(h as u64));
    }
    let max_w = groups[0].saturating_mul(64);
    let max_h = groups[1].saturating_mul(64);
    let min_w = (groups[0] / 2).max(64);
    let min_h = (groups[1] / 2).max(64);
    if w < min_w || w > max_w || h < min_h || h > max_h {
        return None;
    }
    // Console compute shaders commonly use [8, 8, 1] or Wave32 [8, 4, 1] thread groups
    // (e.g. [320, 360, 1] -> 2560x1440 at 8x4 or 1280x720 at 4x2); score by closest fit.
    let d88 = (w as i64 - groups[0].saturating_mul(8) as i64).unsigned_abs()
        + (h as i64 - groups[1].saturating_mul(8) as i64).unsigned_abs();
    let d84 = (w as i64 - groups[0].saturating_mul(8) as i64).unsigned_abs()
        + (h as i64 - groups[1].saturating_mul(4) as i64).unsigned_abs();
    let d42 = (w as i64 - groups[0].saturating_mul(4) as i64).unsigned_abs()
        + (h as i64 - groups[1].saturating_mul(2) as i64).unsigned_abs();
    Some(d88.min(d84).min(d42))
}

fn is_air_volume_uav_format(fmt: i32) -> bool {
    // R16G16B16A16_FLOAT (10), R11G11B10_FLOAT (26)
    matches!(fmt, 10 | 26)
}

/// Match a 3D volumetric inscattering froxel UAV against dispatch grid dimensions, returning
/// a lower-is-better score when compatible (`None` if incompatible).
fn score_volume_target(rd: &D3D12_RESOURCE_DESC, view_fmt: i32, groups: [u32; 3]) -> Option<u64> {
    if rd.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE3D
        || rd.SampleDesc.Count != 1
        || rd.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0
        || !is_air_volume_uav_format(view_fmt)
    {
        return None;
    }
    let w = rd.Width as u32;
    let h = rd.Height;
    let d = rd.DepthOrArraySize as u32;
    if w < 16 || h < 16 || d < 4 {
        return None;
    }
    if groups[0] == 0 || groups[1] <= 1 {
        return Some(u64::MAX - (w as u64).saturating_mul(h as u64).saturating_mul(d as u64));
    }
    let max_w = groups[0].saturating_mul(32);
    let max_h = groups[1].saturating_mul(32);
    let min_w = (groups[0] / 2).max(16);
    let min_h = (groups[1] / 2).max(16);
    if w < min_w || w > max_w || h < min_h || h > max_h {
        return None;
    }
    let dw = (w as i64 - groups[0].saturating_mul(8) as i64)
        .unsigned_abs()
        .min((w as i64 - groups[0].saturating_mul(4) as i64).unsigned_abs());
    let dh = (h as i64 - groups[1].saturating_mul(8) as i64)
        .unsigned_abs()
        .min((h as i64 - groups[1].saturating_mul(4) as i64).unsigned_abs());
    let dd = if groups[2] <= 1 {
        0
    } else {
        (d as i64 - groups[2] as i64)
            .unsigned_abs()
            .min((d as i64 - groups[2].saturating_mul(2) as i64).unsigned_abs())
            .min((d as i64 - groups[2].saturating_mul(4) as i64).unsigned_abs())
            .min((d as i64 - groups[2].saturating_mul(8) as i64).unsigned_abs())
    };
    Some(dw + dh + dd)
}

const MAX_SLOT_INDEX: u32 = 2_000_000;

/// Decode a CBV / buffer `V#` descriptor (`gpu::desc::cbv_words` / `buffer_words`) into
/// `(va, byte_len)` if it is a readable buffer descriptor (`w3 >> 28 < 8` and dst_sel `0xFAC`).
fn decode_cbv_vsharp(w: [u32; 4]) -> Option<(usize, usize)> {
    if (w[3] >> 28) >= 8 || (w[3] & 0xFFF) != 0xFAC {
        return None;
    }
    let va = (w[0] as u64 | ((w[1] as u64 & 0xFFFF) << 32)) as usize;
    if va == 0 {
        return None;
    }
    let stride = ((w[1] >> 16) & 0x3FFF) as usize;
    let records = w[2] as usize;
    let bytes = if stride == 0 { records } else { records.saturating_mul(stride) };
    (bytes >= 4).then_some((va, bytes.min(2048)))
}

/// Resolve a GPU VA to a CPU-readable host address when it lives in a `PAGE_NOACCESS` GPU mirror view.
fn resolve_cbv_va(va: usize) -> usize {
    if va == 0 {
        return 0;
    }
    if crate::gpu::shader::readable(va, 4) {
        return va;
    }
    match crate::hle::kernelx::mirrors::mapped_source(va) {
        Some(Some(backing)) => backing,
        Some(None) => 0,
        None => crate::hle::kernelx::mirrors::source(va).unwrap_or(va),
    }
}

/// Scan up to `max_bytes` (capped at 2048 B) of readable constant-buffer memory at `va`,
/// resolving each chunk (capped at 256 B and never crossing a 64 KiB mirror page boundary)
/// through `resolve`, and feed candidate bindless descriptor indices `< MAX_SLOT_INDEX` into `push_idx`.
unsafe fn scan_cbv_indices_with(
    va: usize,
    max_bytes: usize,
    mut resolve: impl FnMut(usize) -> usize,
    push_idx: &mut impl FnMut(u32),
) {
    if va == 0 || max_bytes < 4 {
        return;
    }
    const PAGE: usize = crate::hle::kernelx::mirrors::PAGE;
    let total = max_bytes.min(2048);
    let mut off = 0usize;
    while off < total {
        let Some(cur_va) = va.checked_add(off) else { break };
        let to_page_end = PAGE - (cur_va & (PAGE - 1));
        let chunk = (total - off).min(256).min(to_page_end);
        let base = resolve(cur_va);
        if base == 0 || !crate::gpu::shader::readable(base, chunk) {
            break;
        }
        let words = chunk / 4;
        for k in 0..words {
            let v = ((base + 4 * k) as *const u32).read_unaligned();
            if v != 0 {
                push_idx(v);
            }
        }
        off += chunk;
    }
}

/// Scan up to `max_bytes` (capped at 2048 B) of readable constant-buffer memory at `va` in
/// page-bounded chunks and feed candidate bindless descriptor indices `< MAX_SLOT_INDEX` into `push_idx`.
unsafe fn scan_cbv_indices(va: usize, max_bytes: usize, push_idx: &mut impl FnMut(u32)) {
    scan_cbv_indices_with(va, max_bytes, resolve_cbv_va, push_idx);
}

/// Inspect a compute dispatch's bound descriptor tables, SM 6.6 directly-indexed view heap
/// (`view_heap_base`), and root/table CBVs to locate its output float UAV (2D screen-space or
/// 3D froxel volume when `want_3d` is true) and optional depth SRV for crease AO.
unsafe fn find_dispatch_views(
    role: PassRole,
    layout: &Layout,
    args: &[u8],
    groups: [u32; 3],
    view_heap_base: u32,
    want_3d: bool,
) -> (Option<(usize, i32, u32, u32, u32)>, Option<(usize, i32, u32, u32)>, usize, usize, usize) {
    let Some(u) = crate::gpu::uma::get() else { return (None, None, 0, 0, 0) };
    let word = |at: usize| args.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let mut param_tables: Vec<u32> = Vec::new();
    let mut tables: Vec<u32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut push_idx = |v: u32| {
        if v < MAX_SLOT_INDEX && seen.insert(v) {
            indices.push(v);
        }
    };
    if view_heap_base != 0 {
        tables.push(view_heap_base);
    }
    for p in &layout.params {
        match *p {
            Param::Constants { at, count } => {
                for k in 0..count.min(64) as usize {
                    if let Some(v) = word(at as usize + 4 * k) {
                        push_idx(v);
                    }
                }
            }
            Param::Descriptor { at } => {
                let lo = word(at as usize).unwrap_or(0) as u64;
                let hi = word(at as usize + 4).unwrap_or(0) as u64;
                let va = (lo | (hi << 32)) as usize;
                scan_cbv_indices(va, 2048, &mut push_idx);
            }
            Param::Table { at } => {
                if let Some(t) = word(at as usize) {
                    if t != 0 {
                        if !param_tables.contains(&t) {
                            param_tables.push(t);
                        }
                        if !tables.contains(&t) {
                            tables.push(t);
                        }
                    }
                }
            }
        }
    }

    // Scan direct slots `0..64` of every bound `Param::Table` for BOTH direct image T# descriptors
    // and CBV V# descriptors (whose buffer contents often carry bindless indices into `view_heap`).
    let mut table_cbvs = 0usize;
    let mut direct_addrs: Vec<usize> = Vec::new();
    for &t in &param_tables {
        for k in 0..64usize {
            let Some(off) = (t as usize).checked_add(32 * k) else { break };
            let Some(addr) = u.base().checked_add(off) else { break };
            if !u.contains(addr) || !crate::gpu::shader::readable(addr, 16) {
                break;
            }
            let w = [
                (addr as *const u32).read_unaligned(),
                ((addr + 4) as *const u32).read_unaligned(),
                ((addr + 8) as *const u32).read_unaligned(),
                ((addr + 12) as *const u32).read_unaligned(),
            ];
            if (w[3] >> 28) >= 8 {
                direct_addrs.push(addr);
            } else if let Some((cbv_va, cbv_len)) = decode_cbv_vsharp(w) {
                table_cbvs += 1;
                scan_cbv_indices(cbv_va, cbv_len, &mut push_idx);
            }
        }
    }

    let mut uav_hit: Option<(u64, usize, i32, u32, u32, u32)> = None;
    let mut depth_hit: Option<(usize, i32, u32, u32)> = None;
    let slots = VIEW_SLOTS.read().unwrap();
    let Some(vs) = slots.as_ref() else { return (None, None, tables.len(), indices.len(), table_cbvs) };

    let mut check_slot = |addr: usize| {
        if !u.contains(addr) || !crate::gpu::shader::readable(addr, 16) {
            return;
        }
        let w0 = (addr as *const u32).read_unaligned();
        let w3 = ((addr + 12) as *const u32).read_unaligned();
        // Image T# descriptors have type >= 8 in bits [31:28] of word 3.
        if (w3 >> 28) < 8 {
            return;
        }
        let Some(&ve) = vs
            .by_addr
            .get(&addr)
            .or_else(|| (w0 != 0).then(|| vs.by_idx.get(&w0)).flatten())
        else {
            return;
        };
        let ptr = ve.res as *mut c_void;
        let Some(res) = ID3D12Resource::from_raw_borrowed(&ptr) else { return };
        let rd = res.GetDesc();
        if ve.is_uav {
            let scored = if want_3d {
                score_volume_target(&rd, ve.fmt, groups).map(|s| (s, rd.DepthOrArraySize as u32))
            } else {
                score_dispatch_target_for_role(role, &rd, ve.fmt, groups).map(|s| (s, 1u32))
            };
            if let Some((score, depth)) = scored {
                if uav_hit.is_none_or(|(best, _, _, _, _, _)| score < best) {
                    uav_hit = Some((score, ve.res, ve.fmt, rd.Width as u32, rd.Height, depth));
                }
            }
        } else if !want_3d
            && depth_hit.is_none()
            && rd.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
            && rd.DepthOrArraySize == 1
            && rd.SampleDesc.Count == 1
            && rd.Width >= 64
            && rd.Height >= 64
        {
            depth_hit = Some((ve.res, ve.fmt, rd.Width as u32, rd.Height));
        }
    };

    for addr in direct_addrs {
        check_slot(addr);
    }
    for &t in &tables {
        for &idx in &indices {
            let Some(off) = (t as usize).checked_add(32usize.saturating_mul(idx as usize)) else { continue };
            let Some(addr) = u.base().checked_add(off) else { continue };
            check_slot(addr);
        }
    }
    (uav_hit.map(|(_, r, f, w, h, d)| (r, f, w, h, d)), depth_hit, tables.len(), indices.len(), table_cbvs)
}

const HLSL: &str = r###"#define RS "RootConstants(num32BitConstants=15, b0), DescriptorTable(SRV(t0)), DescriptorTable(UAV(u0)), UAV(u1), UAV(u2)"
#define CX 64
#define CY 36
#define CELLS (CX * CY)
#define PX 160
#define PY 90
cbuffer Set : register(b0) {
    float level; float constant; float strength; float radius;
    float near_z; float tan_y; uint width; uint height;
    float follow; float most; float air; uint probe;
    float jump; float fresh; float dt_s;
};
Texture2D<float> depth : register(t0);
RWStructuredBuffer<float4> cells : register(u1);
RWStructuredBuffer<float4> points : register(u2);
static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);

float3 seen(int2 p, float2 size) {
    float d = depth.Load(int3(clamp(p, int2(0, 0), int2(size) - 1), 0));
    float z = min(near_z / max(d, 1e-7), 1000.0);
    float2 ndc = (float2(p) + 0.5) / size * 2.0 - 1.0;
    return float3(ndc.x * tan_y * size.x / size.y * z, -ndc.y * tan_y * z, z);
}

#ifdef KEEP
RWTexture2D<float3> radiance : register(u0);
[numthreads(8, 4, 1)]
void keep(uint3 at : SV_DispatchThreadID) {
    uint w, h; radiance.GetDimensions(w, h);
    float3 sum = 0;
    for (uint y = 0; y < 5; y++) for (uint x = 0; x < 5; x++) {
        uint2 p = uint2((float2(at.xy) + (float2(x, y) + 0.5) / 5.0) * float2(w, h) / float2(CX, CY));
        float3 c = max(radiance[p], 0.0);
        c = any(isnan(c)) || any(isinf(c)) ? 0.0 : c;
        sum += c / max(1.0, dot(c, LUMA) / 20.0);
    }
    cells[at.y * CX + at.x] = float4(sum / 25.0, 1.0);
}
[numthreads(1, 1, 1)]
void mean() {
    float3 sum = 0; for (uint i = 0; i < CELLS; i++) sum += cells[i].rgb;
    float4 was = cells[CELLS]; float3 now = sum / CELLS;
    float a = max(dot(now, LUMA), 1e-5), b = max(dot(was.rgb, LUMA), 1e-5);
    bool jumped = jump > 0.0 && max(a / b, b / a) > jump;
    float age = was.a > 0.0 && !jumped ? was.a + dt_s : 0.001;
    cells[CELLS] = float4(was.a > 0.0 ? lerp(was.rgb, now, jumped ? 1.0 : follow) : now, age);
}
#endif

#ifdef PLACE
[numthreads(8, 4, 1)]
void place(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    float3 p = seen(int2((float2(at.xy) + 0.5) * size / float2(CX, CY)), size);
    cells[CELLS + 1 + at.y * CX + at.x] = float4(p, p.z * p.z);
}
#endif

#ifdef GATHER
[numthreads(8, 2, 1)]
void gather(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    float3 p = seen(int2((float2(at.xy) + 0.5) * size / float2(PX, PY)), size);
    float r2 = 0.25 + 0.0025 * p.z * p.z;
    float whole = 0; float3 toward = 0, light = 0, red = 0, green = 0, blue = 0;
    for (uint i = 0; i < CELLS; i++) {
        float4 c = cells[CELLS + 1 + i]; float3 L = cells[i].rgb;
        float3 d = c.xyz - p; float dd = dot(d, d); float w = c.w / (dd + r2);
        float3 dir = d / sqrt(dd + 0.25 * r2);
        whole += w; toward += w * dir; light += w * L;
        red += w * L.r * dir; green += w * L.g * dir; blue += w * L.b * dir;
    }
    whole = max(whole, 1e-6); light /= whole; toward /= whole;
    uint at0 = (at.y * PX + at.x) * 3;
    points[at0] = float4(red / whole - light.r * toward, light.r);
    points[at0 + 1] = float4(green / whole - light.g * toward, light.g);
    points[at0 + 2] = float4(blue / whole - light.b * toward, light.b);
}
#endif

#ifdef AIR
RWTexture3D<float3> scattered : register(u0);
[numthreads(8, 8, 2)]
void clear_air(uint3 at : SV_DispatchThreadID) {
    uint w, h, d; scattered.GetDimensions(w, h, d);
    if (at.x >= w || at.y >= h || at.z >= d) return;
    uint2 q = min(uint2((float2(at.xy) + 0.5) / float2(w, h) * float2(PX, PY)), uint2(PX - 1, PY - 1));
    uint at0 = (q.y * PX + q.x) * 3;
    float3 seen_there = float3(points[at0].w, points[at0 + 1].w, points[at0 + 2].w);
    float far = float(at.z) / float(d);
    float cap = air * lerp(dot(cells[CELLS].rgb, LUMA), dot(seen_there, LUMA), far);
    float3 c = scattered[at]; float has = dot(c, LUMA);
    if (has > cap) scattered[at] = c * (cap / max(has, 1e-6));
}
#endif

#ifdef RAISE
RWTexture2D<uint> light : register(u0);
float3 unpack(uint v) {
    return float3(v & 511, (v >> 9) & 511, (v >> 18) & 511) * exp2(float(v >> 27) - 24.0);
}
uint pack(float3 c) {
    c = clamp(c, 0.0, 65408.0);
    float top = max(c.r, max(c.g, c.b));
    int e = clamp(int(floor(log2(max(top, 1e-9)))) + 16, 0, 31);
    float scale = exp2(float(24 - e));
    if (top * scale >= 511.5) { e = min(e + 1, 31); scale = exp2(float(24 - e)); }
    uint3 m = min(uint3(c * scale + 0.5), 511u);
    return m.r | (m.g << 9) | (m.b << 18) | (uint(e) << 27);
}
[numthreads(8, 8, 1)]
void raise(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    if (at.x >= width || at.y >= height) return;
    int2 p = int2(at.xy);
    float3 c = seen(p, size);
    float3 l = seen(p + int2(-1, 0), size), r = seen(p + int2(1, 0), size);
    float3 u = seen(p + int2(0, -1), size), d = seen(p + int2(0, 1), size);
    float3 dx = abs(l.z - c.z) < abs(r.z - c.z) ? c - l : r - c;
    float3 dy = abs(u.z - c.z) < abs(d.z - c.z) ? c - u : d - c;
    float3 n = normalize(cross(dy, dx) + float3(0, 0, -1e-9));
    if (dot(n, c) > 0.0) n = -n;
    float open = 1.0;
    float px = min(radius * size.y / (2.0 * tan_y * c.z), 128.0);
    float noise = frac(52.9829189 * frac(dot(float2(p), float2(0.06711056, 0.00583715))));
    if (strength > 0.0 && px >= 1.5 && c.z < 400.0) {
        float sum = 0.0; const int N = 12;
        for (int i = 0; i < N; i++) {
            float a = i * 2.399963 + noise * 6.2831853;
            float2 o = float2(cos(a), sin(a)) * (sqrt((i + 0.5) / N) * px);
            float3 v = seen(p + int2(round(o)), size) - c;
            float over = dot(v, n) - 0.004 * c.z - 0.005;
            sum += max(over, 0.0) / sqrt(dot(v, v) + 1e-6) * saturate(1.0 - dot(v, v) / (radius * radius));
        }
        open = saturate(1.0 - strength * 2.0 * sum / N);
    }
    float3 found;
    if (constant > 0.0) {
        found = float3(1.0, 0.98, 0.94) * constant * open;
        light[p] = pack(found); return;
    }
    float2 g = (float2(p) + 0.5) / size * float2(PX, PY) - 0.5 + (noise - 0.5) * 0.5;
    int2 g0 = int2(floor(g)); float2 f = g - float2(g0);
    float4 sr = 0, sg = 0, sb = 0; float sum = 0;
    for (int k = 0; k < 4; k++) {
        int2 q = clamp(g0 + int2(k & 1, k >> 1), int2(0, 0), int2(PX, PY) - 1);
        float z = seen(int2((float2(q) + 0.5) * size / float2(PX, PY)), size).z;
        float2 b = float2(k & 1 ? f.x : 1 - f.x, k >> 1 ? f.y : 1 - f.y);
        float w = (b.x * b.y + 1e-3) / (0.02 + abs(z - c.z) / c.z);
        uint at0 = (q.y * PX + q.x) * 3;
        sr += w * points[at0]; sg += w * points[at0 + 1]; sb += w * points[at0 + 2];
        sum += w;
    }
    float3 all = float3(sr.w, sg.w, sb.w) / sum;
    found = all + 1.2 * float3(dot(sr.xyz, n), dot(sg.xyz, n), dot(sb.xyz, n)) / sum;
    found = clamp(found, 0.2 * all, 2.5 * all);
    found = lerp(found, cells[CELLS].rgb, 0.2) * level * open;
    float3 was = unpack(light[p]);
    if (was.g < 0.5 * min(was.r, was.b) && max(was.r, was.b) > 0.02) was = 0.0;
    float bound = fresh > 0.0 ? lerp(1.0, most, saturate(cells[CELLS].a / fresh)) : most;
    float top = max(dot(found, LUMA) * bound, 1e-6);
    was *= min(1.0, top / max(dot(was, LUMA), 1e-6));
    light[p] = pack(max(was, found));
}
#endif"###;
const CELLS: u64 = 64 * 36;
const CELL_BYTES: u64 = (2 * CELLS + 1) * 16;
const POINT_BYTES: u64 = 160 * 90 * 48;

struct AmbientDevice {
    root: ID3D12RootSignature,
    keep: ID3D12PipelineState,
    mean: ID3D12PipelineState,
    place: ID3D12PipelineState,
    gather: ID3D12PipelineState,
    raise: ID3D12PipelineState,
    air: ID3D12PipelineState,
    cells: ID3D12Resource,
    points: ID3D12Resource,
    heap: ID3D12DescriptorHeap,
    heap_cpu: u64,
    heap_gpu: u64,
    inc: u32,
    next_group: AtomicU32,
    dummy_depth: ID3D12Resource,
}

unsafe impl Send for AmbientDevice {}
unsafe impl Sync for AmbientDevice {}

static DEVICES: Mutex<Option<HashMap<usize, Option<std::sync::Arc<AmbientDevice>>>>> = Mutex::new(None);
const RING_GROUPS: u32 = 64;
const GROUP_SLOTS: u32 = 2;

fn compile_part(part: &str, entry: &[u8]) -> Option<Vec<u8>> {
    let src = format!("#define {}\n{}", part, HLSL);
    unsafe {
        let mut blob = None;
        let r = D3DCompile(
            src.as_ptr() as *const c_void,
            src.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(b"cs_5_1\0".as_ptr()),
            D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &mut blob,
            None,
        );
        match (r, blob) {
            (Ok(()), Some(b)) => Some(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()).to_vec()),
            _ => None,
        }
    }
}

unsafe fn build_device(dev: &ID3D12Device) -> Option<AmbientDevice> {
    let keep_bc = compile_part("KEEP", b"keep\0")?;
    let mean_bc = compile_part("KEEP", b"mean\0")?;
    let place_bc = compile_part("PLACE", b"place\0")?;
    let gather_bc = compile_part("GATHER", b"gather\0")?;
    let raise_bc = compile_part("RAISE", b"raise\0")?;
    let air_bc = compile_part("AIR", b"clear_air\0")?;
    let rs_bc = compile_part("KEEP", b"keep\0")?;
    let dev_raw = dev.as_raw();
    type CreateRsFn = unsafe extern "system" fn(*mut c_void, u32, *const c_void, usize, *const c_void, *mut *mut c_void) -> i32;
    type CreateCsFn = unsafe extern "system" fn(*mut c_void, *const D3D12_COMPUTE_PIPELINE_STATE_DESC, *const c_void, *mut *mut c_void) -> i32;
    type CreateHeapFn = unsafe extern "system" fn(*mut c_void, *const D3D12_DESCRIPTOR_HEAP_DESC, *const c_void, *mut *mut c_void) -> i32;
    type IncSizeFn = unsafe extern "system" fn(*mut c_void, D3D12_DESCRIPTOR_HEAP_TYPE) -> u32;
    type CreateResFn = unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_PROPERTIES, D3D12_HEAP_FLAGS, *const D3D12_RESOURCE_DESC, D3D12_RESOURCE_STATES, *const D3D12_CLEAR_VALUE, *const c_void, *mut *mut c_void) -> i32;
    let create_rs: CreateRsFn = std::mem::transmute(real::orig_fn(dev_raw, 16));
    let create_cs: CreateCsFn = std::mem::transmute(real::orig_fn(dev_raw, 11));
    let create_heap: CreateHeapFn = std::mem::transmute(real::orig_fn(dev_raw, 14));
    let inc_size: IncSizeFn = std::mem::transmute(real::orig_fn(dev_raw, 15));
    let create_res: CreateResFn = std::mem::transmute(real::orig_fn(dev_raw, 27));
    let mut rs_raw = std::ptr::null_mut();
    create_rs(dev_raw, 0, rs_bc.as_ptr() as *const c_void, rs_bc.len(), &ID3D12RootSignature::IID as *const _ as *const c_void, &mut rs_raw);
    if rs_raw.is_null() { return None; }
    let root = ID3D12RootSignature::from_raw(rs_raw);
    let root_sig = root.clone();
    let make = |bc: &[u8]| -> Option<ID3D12PipelineState> {
        let mut d = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: ManuallyDrop::new(Some(root_sig.clone())),
            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: bc.as_ptr() as *const c_void, BytecodeLength: bc.len() },
            ..Default::default()
        };
        let mut raw = std::ptr::null_mut();
        let hr = create_cs(dev_raw, &d, &ID3D12PipelineState::IID as *const _ as *const c_void, &mut raw);
        ManuallyDrop::drop(&mut d.pRootSignature);
        (hr >= 0 && !raw.is_null()).then(|| ID3D12PipelineState::from_raw(raw))
    };
    let hp = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let buf = |bytes: u64| -> Option<ID3D12Resource> {
        let rd = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let mut raw = std::ptr::null_mut();
        let hr = create_res(dev_raw, &hp, D3D12_HEAP_FLAG_NONE, &rd, D3D12_RESOURCE_STATE_COMMON, std::ptr::null(), &ID3D12Resource::IID as *const _ as *const c_void, &mut raw);
        (hr >= 0 && !raw.is_null()).then(|| ID3D12Resource::from_raw(raw))
    };
    let cells = buf(CELL_BYTES)?;
    let points = buf(POINT_BYTES)?;
    let dummy_rd = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: 4,
        Height: 4,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R32_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    let mut dummy_raw = std::ptr::null_mut();
    create_res(dev_raw, &hp, D3D12_HEAP_FLAG_NONE, &dummy_rd, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, std::ptr::null(), &ID3D12Resource::IID as *const _ as *const c_void, &mut dummy_raw);
    let dummy_depth = ID3D12Resource::from_raw(dummy_raw);
    let heap_desc = D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, NumDescriptors: RING_GROUPS * GROUP_SLOTS, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, NodeMask: 0 };
    let mut heap_raw = std::ptr::null_mut();
    create_heap(dev_raw, &heap_desc, &ID3D12DescriptorHeap::IID as *const _ as *const c_void, &mut heap_raw);
    let heap = ID3D12DescriptorHeap::from_raw(heap_raw);
    let heap_cpu = heap.GetCPUDescriptorHandleForHeapStart().ptr as u64;
    let heap_gpu = heap.GetGPUDescriptorHandleForHeapStart().ptr;
    Some(AmbientDevice {
        root,
        keep: make(&keep_bc)?,
        mean: make(&mean_bc)?,
        place: make(&place_bc)?,
        gather: make(&gather_bc)?,
        raise: make(&raise_bc)?,
        air: make(&air_bc)?,
        cells,
        points,
        heap,
        heap_cpu,
        heap_gpu,
        inc: inc_size(dev_raw, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
        next_group: AtomicU32::new(0),
        dummy_depth,
    })
}

impl AmbientDevice {
    fn alloc_group(&self) -> (u64, u64) {
        let g = self.next_group.fetch_add(1, Ordering::Relaxed) % RING_GROUPS;
        let off = (g * GROUP_SLOTS) as u64 * self.inc as u64;
        (self.heap_cpu + off, self.heap_gpu + off)
    }
}

unsafe fn get_device(dev: &ID3D12Device) -> Option<std::sync::Arc<AmbientDevice>> {
    let key = dev.as_raw() as usize;
    {
        let g = DEVICES.lock().unwrap();
        if let Some(entry) = g.as_ref().and_then(|m| m.get(&key)) {
            return entry.clone();
        }
    }
    let built = build_device(dev).map(std::sync::Arc::new);
    DEVICES.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, built.clone());
    built
}

unsafe fn uav_barrier(list: *mut c_void, res_raw: usize) {
    const PC_RESOURCE_BARRIER: usize = 26;
    let b = if res_raw == 0 {
        D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER { pResource: ManuallyDrop::new(None) }),
            },
        }
    } else {
        D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                    pResource: std::mem::transmute_copy(&res_raw),
                }),
            },
        }
    };
    let f: unsafe extern "system" fn(*mut c_void, u32, *const D3D12_RESOURCE_BARRIER) =
        std::mem::transmute(real::orig_fn(list, PC_RESOURCE_BARRIER));
    f(list, 1, &b);
}

pub(super) unsafe fn after_compute_dispatch(
    list: *mut c_void,
    pso_resolved: usize,
    groups: [u32; 3],
    layout: Option<&Layout>,
    args: &[u8],
    view_heap_base: u32,
) {
    if !has_compute_psos() || !active() {
        return;
    }
    let Some(role) = role_of(pso_resolved) else { return };
    let (level, constant) = match set() {
        Set::Off => return,
        Set::Estimate(v) => (v, 0.0f32),
        Set::Constant(v) => (0.0, v),
    };
    if role != PassRole::RadianceCache && constant == 0.0 && !KEPT.load(Ordering::Relaxed) {
        return;
    }
    if role == PassRole::InscatteringVolume && (constant > 0.0 || air() == 0.0) {
        return;
    }
    let Some(layout) = layout else { return };
    let ty = list_x::tracked_list_type(list);
    if !matches!(ty, Some(D3D12_COMMAND_LIST_TYPE_DIRECT | D3D12_COMMAND_LIST_TYPE_COMPUTE)) {
        return;
    }
    let Some(cl) = ID3D12GraphicsCommandList::from_raw_borrowed(&list) else { return };
    let mut dev: Option<ID3D12Device> = None;
    if cl.GetDevice(&mut dev).is_err() { return; }
    let Some(dev) = dev else { return };
    let Some(ad) = get_device(&dev) else { return };
    let want_3d = role == PassRole::InscatteringVolume;
    let (uav_opt, depth_opt, _, _, _) = find_dispatch_views(role, layout, args, groups, view_heap_base, want_3d);
    let Some((uav_res, uav_fmt, w, h, d)) = uav_opt else { return };
    let (cpu0, gpu0) = ad.alloc_group();
    let inc = ad.inc as u64;
    let dev_raw = dev.as_raw();
    type CreateSrvFn = unsafe extern "system" fn(*mut c_void, *mut c_void, *const D3D12_SHADER_RESOURCE_VIEW_DESC, usize);
    type CreateUavFn = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const D3D12_UNORDERED_ACCESS_VIEW_DESC, usize);
    let create_srv: CreateSrvFn = std::mem::transmute(real::orig_fn(dev_raw, 18));
    let create_uav: CreateUavFn = std::mem::transmute(real::orig_fn(dev_raw, 19));
    let (depth_raw, depth_fmt, depth_w, depth_h, has_depth) = match (role, depth_opt) {
        (PassRole::TemporalResolve, Some((r, f, dw, dh))) => (r as *mut c_void, DXGI_FORMAT(f), dw, dh, 1u32),
        _ => (ad.dummy_depth.as_raw(), DXGI_FORMAT_R32_FLOAT, 1, 1, 0u32),
    };
    let depth_srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: depth_fmt,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
    };
    let out_uav_desc = if want_3d {
        D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT(uav_fmt),
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE3D,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture3D: D3D12_TEX3D_UAV { MipSlice: 0, FirstWSlice: 0, WSize: d } },
        }
    } else if uav_fmt == 42 {
        D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_UINT,
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_UAV { MipSlice: 0, PlaneSlice: 0 } },
        }
    } else {
        D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT(uav_fmt),
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_UAV { MipSlice: 0, PlaneSlice: 0 } },
        }
    };
    create_srv(dev_raw, depth_raw, &depth_srv, cpu0 as usize);
    create_uav(dev_raw, uav_res as *mut c_void, std::ptr::null_mut(), &out_uav_desc, (cpu0 + inc) as usize);
    let heap_raw = ad.heap.as_raw() as usize;
    let set_heaps: unsafe extern "system" fn(*mut c_void, u32, *const usize) = std::mem::transmute(real::orig_fn(list, 28));
    let set_cs_rs: unsafe extern "system" fn(*mut c_void, *mut c_void) = std::mem::transmute(real::orig_fn(list, 29));
    let set_pso: unsafe extern "system" fn(*mut c_void, *mut c_void) = std::mem::transmute(real::orig_fn(list, 25));
    let set_cs_table: unsafe extern "system" fn(*mut c_void, u32, u64) = std::mem::transmute(real::orig_fn(list, 31));
    let set_cs_uav: unsafe extern "system" fn(*mut c_void, u32, u64) = std::mem::transmute(real::orig_fn(list, 33));
    let set_cs_consts: unsafe extern "system" fn(*mut c_void, u32, u32, *const u32, u32) = std::mem::transmute(real::orig_fn(list, 35));
    let dispatch: unsafe extern "system" fn(*mut c_void, u32, u32, u32) = std::mem::transmute(real::orig_fn(list, 14));
    let [strength, radius] = shade();
    let settle_cfg = settle();
    let mut words = [0u32; 15];
    let floats = [level, constant, strength, radius, 0.1f32, 0.45f32];
    for (w, f) in words.iter_mut().zip(floats) { *w = f.to_bits(); }
    words[6] = w; words[7] = h;
    words[8] = (if role == PassRole::RadianceCache { follow_radiance() } else { 0.04 }).to_bits();
    words[9] = 4.0f32.to_bits();
    words[10] = air().to_bits();
    words[11] = 0;
    words[12] = settle_cfg.map_or(0.0, |s| s.jump).to_bits();
    words[13] = settle_cfg.map_or(0.0, |s| s.fresh).to_bits();
    words[14] = frame_ms_radiance().to_bits();
    set_heaps(list, 1, &heap_raw);
    set_cs_rs(list, ad.root.as_raw());
    set_cs_consts(list, 0, 15, words.as_ptr(), 0);
    set_cs_table(list, 1, gpu0);
    set_cs_table(list, 2, gpu0 + inc);
    set_cs_uav(list, 3, ad.cells.GetGPUVirtualAddress());
    set_cs_uav(list, 4, ad.points.GetGPUVirtualAddress());
    let run = |pso: &ID3D12PipelineState, x, y, z| {
        set_pso(list, pso.as_raw());
        dispatch(list, x, y, z);
        uav_barrier(list, 0);
    };
    match role {
        PassRole::RadianceCache => {
            run(&ad.keep, 8, 9, 1);
            run(&ad.mean, 1, 1, 1);
            KEPT.store(true, Ordering::Relaxed);
        }
        PassRole::InscatteringVolume => run(&ad.air, w.div_ceil(8), h.div_ceil(8), d.div_ceil(2)),
        PassRole::TemporalResolve => {
            if constant == 0.0 {
                if has_depth == 0 { return; }
                run(&ad.place, 8, 9, 1);
                run(&ad.gather, 20, 45, 1);
            }
            run(&ad.raise, w.div_ceil(8), h.div_ceil(8), 1);
        }
    }
    let _ = depth_w;
    let _ = depth_h;
    list_x::restore_after_internal_pass(list, false, true);
}

#[inline]
pub(super) fn has_sky_psos() -> bool {
    false
}

pub(super) unsafe fn after_draw(_list: *mut c_void, _pso_resolved: usize) {}

pub(super) fn note_graphics_pso(_pso: usize, _ps: Option<&[u8]>) {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings() {
        assert_eq!(parse_set(""), Set::Off);
        assert_eq!(parse_set("0.6"), Set::Estimate(0.6));
        assert_eq!(parse_set("constant:0.3"), Set::Constant(0.3));
    }
    #[test]
    fn roles_by_name() {
        assert_eq!(classify_cs(b"CS_ScreenProbes_TemporalResolve_x"), Some(PassRole::TemporalResolve));
        assert_eq!(classify_cs(b"CS_CacheScreenSpaceRadiance_x"), Some(PassRole::RadianceCache));
        assert_eq!(classify_cs(b"CS_AccumulateLocalLightInscatteringVolume_x"), Some(PassRole::InscatteringVolume));
    }
}
