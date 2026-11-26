//! GPU-side constant/buffer fills before lists run (`XCL_CARD_STEPS=1|2`). Default off.
//!
//! Pareto env (see `scripts/pareto-amd.ps1`): `XCL_RELAY=2`, `XCL_CARD_STEPS=2`,
//! `XCL_THIN=1`, `XCL_JOIN_PARTS=8`, `XCL_ASYNC_SUBMIT=1`, `XCL_LAND=1`, `XCL_WIDE_BACK=1`,
//! `XCL_INDIRECT_BATCH=2`, `XCL_INDIRECT_WINDOW=1`, `XCL_AMBIENT=0.8`, `XCL_AMBIENT_AIR=1.5`,
//! `XCL_PORT_CONSTANTS=1`, `XCL_PORT_CONSTANTS_ORDER=1`, renderer ports 2/5/6/7 as needed.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;
use super::list_x;
use super::real;

static SKIPPED: AtomicU64 = AtomicU64::new(0);
static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static COPIES: AtomicU64 = AtomicU64::new(0);
static GAP_LOGGED: OnceLock<()> = OnceLock::new();
static ENABLED_LOGGED: OnceLock<()> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
struct CopyStep {
    dst: usize,
    src: usize,
    len: usize,
}

static PENDING: Mutex<VecDeque<CopyStep>> = Mutex::new(VecDeque::new());

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

fn drain_pending() -> Vec<CopyStep> {
    let mut q = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if q.is_empty() {
        return Vec::new();
    }
    std::mem::take(&mut *q).into()
}

/// Called before title lists execute; returns true when a card-step prelude list was submitted.
pub fn try_prep_before_lists(queue: *mut c_void, _mapped: &[*mut c_void]) -> bool {
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
            "D3D12.X: XCL_CARD_STEPS={st} GPU prelude enabled (deferred buffer copies before title lists)"
        );
    });

    let ops = drain_pending();
    if ops.is_empty() {
        return false;
    }
    if st < 2 {
        // Stage 1 constant fills are not queued on this path yet; copies still require stage 2.
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    let ok = unsafe { submit_copies(queue, &ops) };
    if ok {
        SUBMITTED.fetch_add(1, Ordering::Relaxed);
        COPIES.fetch_add(ops.len() as u64, Ordering::Relaxed);
    } else {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        for op in ops {
            let _ = defer_copy_to_prelude(op.dst, op.src, op.len);
        }
    }
    ok
}

unsafe fn submit_copies(queue: *mut c_void, ops: &[CopyStep]) -> bool {
    if queue.is_null() || ops.is_empty() {
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
        let (alloc, list, list_raw) = take_list(&dev, dev_key, ty)?;
        for op in ops {
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
    let skip = SKIPPED.swap(0, Ordering::Relaxed) as f64 / frames;
    if submitted > 0.0 || skip > 0.0 {
        crate::xlog_info!(
            "D3D12.X: card_steps (XCL_CARD_STEPS={}): {:.1} GPU preludes/frame, {:.0} copies/frame{skip}",
            stage(),
            submitted,
            copies,
            skip = if skip > 0.0 { format!(", {:.0} skipped/frame", skip) } else { String::new() },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continues_adjacent_non_overlapping() {
        assert!(continues((0x100, 0x200, 64), (0x140, 0x240, 32)));
        assert!(!continues((0x100, 0x200, 64), (0x120, 0x220, 32)));
    }
}