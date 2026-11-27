//! GPU-side constant/buffer fills before lists run (`XCL_CARD_STEPS=1|2`). Default off.
//!
//! Play env (see `scripts/play-env-amd.ps1`): `XCL_RELAY=2`, `XCL_CARD_STEPS=2`,
//! `XCL_THIN=1`, `XCL_JOIN_PARTS=8`, `XCL_ASYNC_SUBMIT=1`, `XCL_LAND=1`, `XCL_WIDE_BACK=1`,
//! `XCL_INDIRECT_BATCH=2`, `XCL_INDIRECT_WINDOW=1`, `XCL_AMBIENT=0.8`, `XCL_AMBIENT_AIR=1.5`,
//! `XCL_PORT_CONSTANTS=1`, `XCL_PORT_CONSTANTS_ORDER=1`, renderer ports 2/5/6/7 as needed.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use super::list_x;
use super::real;

static SKIPPED: AtomicU64 = AtomicU64::new(0);
static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static COPIES: AtomicU64 = AtomicU64::new(0);
static FILLS: AtomicU64 = AtomicU64::new(0);
static GAP_LOGGED: OnceLock<()> = OnceLock::new();
static ENABLED_LOGGED: OnceLock<()> = OnceLock::new();

const STAGING_BYTES: u64 = 4 << 20;

#[derive(Clone, Copy, Debug)]
struct CopyStep {
    dst: usize,
    src: usize,
    len: usize,
}

#[derive(Clone, Copy, Debug)]
struct FillStep {
    src: usize,
    len: usize,
    slot_cpu: u64,
}

static PENDING: Mutex<VecDeque<CopyStep>> = Mutex::new(VecDeque::new());
static PENDING_FILLS: Mutex<VecDeque<FillStep>> = Mutex::new(VecDeque::new());

struct StagingBuf {
    buf: ID3D12Resource,
    gpu: u64,
    dev_key: usize,
}

static STAGING: Mutex<Option<StagingBuf>> = Mutex::new(None);

struct Own {
    alloc: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    dev_key: usize,
    ty: D3D12_COMMAND_LIST_TYPE,
}

struct Pool {
    free: Vec<Own>,
}

static POOL: Mutex<Pool> = Mutex::new(Pool { free: Vec::new() });

/// `0` off, `1` fills only, `2` fills + buffer copies (needs [`super::relay::exact`]).
pub fn stage() -> u32 {
    super::port::card_steps_stage()
}

/// `XCL_JOIN_COPIES=1`: adjacent deferred copies become one step.
pub fn join_copies() -> bool {
    super::port::join_copies()
}

/// `XCL_CARD_REHASH_RUNS=1`: re-hash compare ranges across runs in one pass.
pub fn rehash_runs() -> bool {
    super::port::card_rehash_runs()
}

/// Join sparse chunk compare/upload runs when [`rehash_runs`].
pub fn join_compare_runs(runs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    super::card_rehash::join_runs(runs)
}

/// Whether `next` continues `last` as one contiguous copy (dest/source both advance).
pub fn continues(last: (usize, usize, usize), next: (usize, usize, usize)) -> bool {
    let ((d, s, n), (dst, src, len)) = (last, next);
    let Some(all) = n.checked_add(len) else {
        return false;
    };
    d.checked_add(n) == Some(dst)
        && s.checked_add(n) == Some(src)
        && (d.checked_add(all).is_some_and(|end| end <= s) || s.checked_add(all).is_some_and(|end| end <= d))
}

/// Stage 2: queue a UMA copy for the GPU prelude instead of recording it on the title list.
pub fn defer_copy_to_prelude(dst: usize, src: usize, len: usize) -> bool {
    if stage() < 2 || !super::relay::exact() {
        return false;
    }
    if len == 0 || len % 4 != 0 {
        return false;
    }
    if real::uma_window_buffer(dst, len).is_none() || real::uma_window_buffer(src, len).is_none() {
        return false;
    }
    let mut q = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if join_copies() {
        if let Some(last) = q.back().copied() {
            if continues((last.dst, last.src, last.len), (dst, src, len)) {
                if let Some(back) = q.back_mut() {
                    back.len = back.len.saturating_add(len);
                    return true;
                }
            }
        }
    }
    q.push_back(CopyStep { dst, src, len });
    true
}

fn fill_eligible(src: usize, len: usize) -> bool {
    if len == 0 || len > 64 * 1024 || !len.is_multiple_of(16) {
        return false;
    }
    if !super::port::constants() {
        return false;
    }
    if super::port_constants::on() && !super::port_constants::touches_range(src, len) {
        return false;
    }
    real::uma_window_buffer(src, len).is_some()
}

/// Stage 1+: queue a constant-buffer fill for the GPU prelude (CBV retargeted at staging).
pub fn defer_fill_to_prelude(src: usize, len: usize, slot_cpu: u64) -> bool {
    if stage() < 1 || !super::relay::exact() {
        return false;
    }
    if !fill_eligible(src, len) {
        return false;
    }
    let mut q = PENDING_FILLS.lock().unwrap_or_else(|e| e.into_inner());
    q.push_back(FillStep { src, len, slot_cpu });
    true
}

fn drain_pending() -> Vec<CopyStep> {
    let mut q = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if q.is_empty() {
        return Vec::new();
    }
    std::mem::take(&mut *q).into()
}

fn drain_fills() -> Vec<FillStep> {
    let mut q = PENDING_FILLS.lock().unwrap_or_else(|e| e.into_inner());
    if q.is_empty() {
        return Vec::new();
    }
    std::mem::take(&mut *q).into()
}

/// Called before title lists execute; returns true when a card-step prelude list was submitted.
pub fn try_prep_before_lists(queue: *mut c_void, _mapped: &[*mut c_void]) -> bool {
    // `XCL_LAZY_PRELUDE=0`: still build when stage is on, but skip empty-queue early-out so a
    // prelude list is prepared with the first submit that needs one.
    let _lazy = super::render_env::lazy_prelude();
    let st = stage();
    if st == 0 {
        return false;
    }
    if !super::relay::exact() {
        GAP_LOGGED.get_or_init(|| {
            crate::xlog_warn!(
                "D3D12.X: XCL_CARD_STEPS={st} needs XCL_RELAY=2; card-step GPU lists not submitted"
            );
        });
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
  ENABLED_LOGGED.get_or_init(|| {
        crate::xlog_info!(
            "D3D12.X: XCL_CARD_STEPS={st} GPU prelude enabled (constant fills / deferred copies before title lists)"
        );
    });

    let fills = if st >= 1 { drain_fills() } else { Vec::new() };
    let copies = if st >= 2 { drain_pending() } else { Vec::new() };
    if fills.is_empty() && copies.is_empty() {
        return false;
    }

    let ok = unsafe { submit_prelude(queue, &fills, &copies) };
    if ok {
        SUBMITTED.fetch_add(1, Ordering::Relaxed);
        if !fills.is_empty() {
            FILLS.fetch_add(fills.len() as u64, Ordering::Relaxed);
        }
        if !copies.is_empty() {
            COPIES.fetch_add(copies.len() as u64, Ordering::Relaxed);
        }
    } else {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        for op in copies {
            let _ = defer_copy_to_prelude(op.dst, op.src, op.len);
        }
        for op in fills {
            let _ = defer_fill_to_prelude(op.src, op.len, op.slot_cpu);
        }
    }
    ok
}

unsafe fn staging(dev: &ID3D12Device, dev_key: usize) -> Result<(), String> {
    let mut g = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = g.as_ref() {
        if s.dev_key == dev_key {
            return Ok(());
        }
    }
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: STAGING_BYTES,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut buf: Option<ID3D12Resource> = None;
    dev.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut buf)
        .map_err(|e| format!("card staging buffer: {e}"))?;
    let buf = buf.ok_or("card staging buffer missing")?;
    let gpu = buf.GetGPUVirtualAddress();
    *g = Some(StagingBuf { buf, gpu, dev_key });
    Ok(())
}

unsafe fn submit_prelude(queue: *mut c_void, fills: &[FillStep], copies: &[CopyStep]) -> bool {
    if queue.is_null() || (fills.is_empty() && copies.is_empty()) {
        return false;
    }
    let Some(queue) = ID3D12CommandQueue::from_raw_borrowed(&queue) else {
        return false;
    };
    let mut dev: Option<ID3D12Device> = None;
    if queue.GetDevice(&mut dev).is_err() {
        return false;
    };
    let Some(dev) = dev else {
        return false;
    };
    let dev_key = dev.as_raw() as usize;
    let ty = queue.GetDesc().Type;
    if ty != D3D12_COMMAND_LIST_TYPE_DIRECT && ty != D3D12_COMMAND_LIST_TYPE_COPY {
        return false;
    }

    let run = (|| -> Result<(), String> {
        if !fills.is_empty() {
            staging(&dev, dev_key)?;
        }
        let staging_raw = STAGING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.buf.as_raw() as usize);
        let staging_gpu = STAGING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.gpu).unwrap_or(0);
        let (alloc, list, list_raw) = take_list(&dev, dev_key, ty)?;
        let mut off = 0u64;
        let mut planned: Vec<(FillStep, u64)> = Vec::with_capacity(fills.len());
        for fill in fills {
            let len = fill.len as u64;
            if off + len > STAGING_BYTES {
                return Err("card prelude staging overflow".into());
            }
            let Some(staging_raw) = staging_raw else {
                return Err("fill staging missing".into());
            };
            let Some((s, soff)) = real::uma_window_buffer(fill.src, fill.len) else {
                return Err("fill no longer in UMA".into());
            };
            list_x::transition(list_raw, s, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_SOURCE);
            list_x::as_copy_dest(list_raw, staging_raw, || {
                list_x::copy_buffer_region(list_raw, staging_raw, off, s, soff, len);
            });
            list_x::transition(list_raw, s, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON);
            planned.push((*fill, off));
            off = (off + len).next_multiple_of(256);
        }
        for op in copies {
            let (Some((d, doff)), Some((s, soff))) = (
                real::uma_window_buffer(op.dst, op.len),
                real::uma_window_buffer(op.src, op.len),
            ) else {
                return Err("copy no longer in UMA windows".into());
            };
            if d == s {
                return Err("same-window copy not supported in card prelude".into());
            }
            list_x::transition(list_raw, s, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_SOURCE);
            list_x::as_copy_dest(list_raw, d, || {
                list_x::copy_buffer_region(list_raw, d, doff, s, soff, op.len as u64);
            });
            list_x::transition(list_raw, s, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON);
        }
        list.Close().map_err(|e| format!("card prelude Close: {e}"))?;
        let fence: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(|e| format!("fence: {e}"))?;
        let value = 1u64;
        queue.ExecuteCommandLists(&[Some(list.clone().into())]);
        queue.Signal(&fence, value).map_err(|e| format!("Signal: {e}"))?;
        crate::gpu::fence::wait(&fence, value, &dev, None, 30_000)
            .map_err(|e| format!("card prelude wait: {e}"))?;
        for (fill, at) in planned {
            if let Some(slot) = super::console_heap::slot(fill.slot_cpu).filter(|s| !s.sampler) {
                super::console_heap::write(&slot, &crate::gpu::desc::cbv_words(staging_gpu + at, fill.len as u32));
            }
        }
        return_list(dev_key, ty, alloc, list);
        Ok(())
    })();

    match run {
        Ok(()) => true,
        Err(e) => {
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            if n < 4 {
                crate::xlog_warn!("D3D12.X: card_steps GPU prelude failed: {e}");
            }
            false
        }
    }
}

unsafe fn take_list(
    dev: &ID3D12Device,
    dev_key: usize,
    ty: D3D12_COMMAND_LIST_TYPE,
) -> Result<(ID3D12CommandAllocator, ID3D12GraphicsCommandList, *mut c_void), String> {
    {
        let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = pool.free.iter().position(|e| e.dev_key == dev_key && e.ty == ty) {
            let own = pool.free.swap_remove(i);
            own.alloc.Reset().map_err(|e| format!("card alloc reset: {e}"))?;
            own.list.Reset(&own.alloc, None).map_err(|e| format!("card list reset: {e}"))?;
            let raw = own.list.as_raw();
            return Ok((own.alloc, own.list, raw));
        }
    }
    let alloc: ID3D12CommandAllocator = dev.CreateCommandAllocator(ty).map_err(|e| format!("card alloc: {e}"))?;
    let _ = alloc.SetName(windows::core::w!("xcl card-steps alloc"));
    let list: ID3D12GraphicsCommandList = dev.CreateCommandList(0, ty, &alloc, None).map_err(|e| format!("card list: {e}"))?;
    let _ = list.SetName(windows::core::w!("xcl card-steps"));
    let raw = list.as_raw();
    list_x::on_create(raw, alloc.as_raw() as usize, 0, ty);
    let _ = list.Close();
    list.Reset(&alloc, None).map_err(|e| format!("card list reset: {e}"))?;
    Ok((alloc, list, raw))
}

unsafe fn return_list(dev_key: usize, ty: D3D12_COMMAND_LIST_TYPE, alloc: ID3D12CommandAllocator, list: ID3D12GraphicsCommandList) {
    POOL.lock().unwrap_or_else(|e| e.into_inner()).free.push(Own { alloc, list, dev_key, ty });
}

pub fn log_stats(frames: f64) {
    if stage() == 0 || frames <= 0.0 {
        return;
    }
    let submitted = SUBMITTED.swap(0, Ordering::Relaxed) as f64 / frames;
    let copies = COPIES.swap(0, Ordering::Relaxed) as f64 / frames;
    let fills = FILLS.swap(0, Ordering::Relaxed) as f64 / frames;
    let skip = SKIPPED.swap(0, Ordering::Relaxed) as f64 / frames;
    if submitted > 0.0 || skip > 0.0 {
        crate::xlog_info!(
            "D3D12.X: card_steps (XCL_CARD_STEPS={}): {:.1} GPU preludes/frame, {:.0} fills/frame, {:.0} copies/frame{skip}",
            stage(),
            submitted,
            fills,
            copies,
            skip = if skip > 0.0 { format!(", {:.0} skipped/frame", skip) } else { String::new() },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Acceptance: stage-2 copies and stage-1 fills both require `XCL_RELAY=2` ([`crate::hle::d3d12x::relay::exact`]).
    /// Without it, prelude deferrals refuse so title lists keep recording the ops themselves.
    #[test]
    fn relay_exact_gates_prelude_deferrals() {
        // When XCL_RELAY≠2 or CARD_STEPS stage is below the path, deferrals refuse.
        // (Parent shells may latch XCL_RELAY; only assert the refuse side when gates are off.)
        if !crate::hle::d3d12x::relay::exact() || stage() < 2 {
            assert!(!defer_copy_to_prelude(0x1000, 0x2000, 64));
        }
        if !crate::hle::d3d12x::relay::exact() || stage() < 1 {
            assert!(!defer_fill_to_prelude(0x1000, 256, 0));
        }
    }

    #[test]
    fn continues_rejects_overlap_and_gap() {
        assert!(!continues((0x100, 0x200, 64), (0x120, 0x240, 32)), "dst overlap");
        assert!(!continues((0x100, 0x200, 64), (0x180, 0x280, 32)), "gap in dst");
        assert!(!continues((0x100, 0x200, 64), (0x140, 0x300, 32)), "gap in src");
    }

    #[test]
    fn continues_adjacent_non_overlapping() {
        assert!(continues((0x100, 0x200, 64), (0x140, 0x240, 32)));        assert!(!continues((0x100, 0x200, 64), (0x120, 0x220, 32)));
    }
}