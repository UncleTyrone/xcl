//! Phase 3.3: when the device reports DXR (`XCL_RAYTRACING` / config `raytracing`), title
//! BLAS/TLAS builds are forwarded to `BuildRaytracingAccelerationStructure` instead of
//! being dropped. Input ranges outside title allocations are rejected and counted.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::OnceLock;

use windows::Win32::Graphics::Direct3D12::*;

static HOST_RT_TIER: AtomicI32 = AtomicI32::new(-1);
static EFFECTIVE_RT_TIER: AtomicI32 = AtomicI32::new(-1);
static REJECTED: AtomicU64 = AtomicU64::new(0);
static FORWARDED: AtomicU64 = AtomicU64::new(0);

/// Host GPU tier from the first successful `D3D12_OPTIONS5` query (before override).
pub fn note_host_raytracing_tier(tier: i32) {
    if HOST_RT_TIER.swap(tier, Ordering::Relaxed) == -1 {
        crate::xlog_info!("D3D12.X: host RaytracingTier {tier}");
    }
}

/// Effective tier after `config.raytracing` / `XCL_RAYTRACING` (`real::adjust_raytracing_options5`).
pub fn note_effective_raytracing_tier(tier: i32) {
    EFFECTIVE_RT_TIER.store(tier, Ordering::Relaxed);
}

/// Whether DXR builds should run (tier advertised to the title is not `NOT_SUPPORTED`).
pub fn active() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        if std::env::var("XCL_RAYTRACING").is_ok_and(|v| {
            let s = v.trim();
            s == "1" || s.eq_ignore_ascii_case("tier1_0") || s.eq_ignore_ascii_case("tier1_1") || s == "10" || s == "11"
        }) {
            return true;
        }
        if std::env::var("XCL_RAYTRACING").is_ok_and(|v| v.trim() == "0" || v.eq_ignore_ascii_case("off")) {
            return false;
        }
        let tier = EFFECTIVE_RT_TIER.load(Ordering::Relaxed);
        if tier >= 0 {
            return tier > 0;
        }
        HOST_RT_TIER.load(Ordering::Relaxed) > 0
    })
}

/// Console BVH builder passes (`DispatchX` with `USE_THREAD_DIMENSIONS`): opt-in only
/// (`XCL_BVH_BUILDER=1`). PC DXR BLAS/TLAS forwarding stays on [`active()`] and is unrelated.
pub fn builder_enabled() -> bool {
    static BVH: OnceLock<bool> = OnceLock::new();
    *BVH.get_or_init(|| std::env::var("XCL_BVH_BUILDER").is_ok_and(|v| v.trim() == "1"))
}

fn ptr_u64<T>(p: *const T) -> u64 {
    p.cast::<()>() as usize as u64
}

fn resolved_bytes(va: u64, len: u64) -> bool {
    if va == 0 || len == 0 {
        return false;
    }
    let len_usize = len.min(usize::MAX as u64) as usize;
    if let Some(cpu) = super::real::resolve_uma_va(va, len_usize) {
        return crate::gpu::shader::readable(cpu as usize, len_usize);
    }
    super::sparse::gpu_view::base_for(va as usize, len_usize)
        .is_some_and(|base| crate::gpu::shader::readable(base, len_usize))
}

fn note_rejected() {
    let n = REJECTED.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n.is_power_of_two() {
        crate::xlog_warn!("D3D12.X: raytracing build rejected ({n} so far; input outside title memory)");
    }
}

fn note_forwarded() {
    FORWARDED.fetch_add(1, Ordering::Relaxed);
}

/// Validate every GPU VA referenced by a PC build description.
unsafe fn validate_desc(desc: &D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC) -> bool {
    if !resolved_bytes(desc.DestAccelerationStructureData, 256) {
        return false;
    }
    if desc.ScratchAccelerationStructureData != 0 {
        let scratch = desc.ScratchAccelerationStructureData;
        if !resolved_bytes(scratch, 256) {
            return false;
        }
    }
    if desc.SourceAccelerationStructureData != 0 && !resolved_bytes(desc.SourceAccelerationStructureData, 256) {
        return false;
    }
    let inputs = &desc.Inputs;
    if inputs.NumDescs == 0 {
        return true;
    }
    match inputs.Type {
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL => {
            let (stride, base) = if inputs.DescsLayout == D3D12_ELEMENTS_LAYOUT_ARRAY {
                (
                    std::mem::size_of::<D3D12_RAYTRACING_GEOMETRY_DESC>() as u64,
                    ptr_u64(inputs.Anonymous.pGeometryDescs),
                )
            } else {
                (8, ptr_u64(inputs.Anonymous.pGeometryDescs))
            };
            let total = inputs.NumDescs as u64 * stride;
            if !resolved_bytes(base, total) {
                return false;
            }
            for i in 0..inputs.NumDescs as u64 {
                let g = if inputs.DescsLayout == D3D12_ELEMENTS_LAYOUT_ARRAY {
                    base + i * stride
                } else {
                    if !resolved_bytes(base + i * 8, 8) {
                        return false;
                    }
                    u64::from_le_bytes(std::slice::from_raw_parts((base + i * 8) as *const u8, 8).try_into().unwrap())
                };
                if g == 0 {
                    continue;
                }
                if !resolved_bytes(g, std::mem::size_of::<D3D12_RAYTRACING_GEOMETRY_DESC>() as u64) {
                    return false;
                }
            }
            true
        }
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL => {
            let per = if inputs.DescsLayout == D3D12_ELEMENTS_LAYOUT_ARRAY {
                std::mem::size_of::<D3D12_RAYTRACING_INSTANCE_DESC>() as u64
            } else {
                8
            };
            let base = inputs.Anonymous.InstanceDescs;
            let total = inputs.NumDescs as u64 * per;
            if !resolved_bytes(base, total) {
                return false;
            }
            if inputs.DescsLayout == D3D12_ELEMENTS_LAYOUT_ARRAY_OF_POINTERS {
                for i in 0..inputs.NumDescs as u64 {
                    if !resolved_bytes(base + i * 8, 8) {
                        return false;
                    }
                    let ptr = u64::from_le_bytes(std::slice::from_raw_parts((base + i * 8) as *const u8, 8).try_into().unwrap());
                    if ptr != 0
                        && !resolved_bytes(ptr, std::mem::size_of::<D3D12_RAYTRACING_INSTANCE_DESC>() as u64)
                    {
                        return false;
                    }
                }
            }
            true
        }
        _ => false,
    }
}

pub fn tick_log() {
    if !active() {
        return;
    }
    let rej = REJECTED.swap(0, Ordering::Relaxed);
    let ok = FORWARDED.swap(0, Ordering::Relaxed);
    if rej > 0 || ok > 0 {
        crate::xlog_info!("D3D12.X: raytracing builds forwarded {ok}, rejected {rej}");
    }
}

type BuildRtFn = unsafe extern "system" fn(
    *mut c_void,
    *const D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC,
    u32,
    *const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC,
);

pub(super) unsafe extern "system" fn cl_build_raytracing_acceleration_structure(
    this: *mut c_void,
    desc: *const D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC,
    num_post_build: u32,
    post_build: *const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC,
) {
    if !active() {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n.is_power_of_two() {
            crate::xlog_info!(
                "D3D12.X: BuildRaytracingAccelerationStructure left out ({n} so far; enable DXR via XCL_RAYTRACING or host tier)"
            );
        }
        return;
    }
    if desc.is_null() {
        note_rejected();
        return;
    }
    let d = desc.read();
    if !validate_desc(&d) {
        note_rejected();
        return;
    }
    let slot = super::layout::slot(
        super::layout::find(super::real::Kind::CommandList.base()).unwrap(),
        "BuildRaytracingAccelerationStructure",
    )
    .unwrap();
    let f: BuildRtFn = std::mem::transmute(crate::hle::d3d12x::real::orig_fn(this, slot));
    f(this, desc, num_post_build, post_build);
    note_forwarded();
    super::real::note_bvh_builder_dispatched();
}