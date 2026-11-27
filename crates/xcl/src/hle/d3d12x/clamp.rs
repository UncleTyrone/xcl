//! Indirect ExecuteIndirect clamp policy (CGBJ Phase 14): batch and window modes beside
//! prelude/inline guards in `bind.rs`.
//!
//! `XCL_INDIRECT_BATCH` — `0`: each clamp on its own (or windows only); `1` (default): batched
//! prelude hoisting; `2`: a compute dispatch does not close the batch until a barrier, wait, or
//! draw that may write argument buffers (as `VICTORY_INDIRECT_BATCH=2`).
//!
//! `XCL_INDIRECT_WINDOW` — `1`: consecutive single-record indirect mesh draws that step through one
//! argument buffer are hoisted as one prelude draw-guard job (up to 64 records); off by default.
//!
//! `XCL_INDIRECT_REST` — unset/`0` (default): an oversized indirect record in a batched pass takes
//! no budget and later records that fit still run (CGBJ `VICTORY_INDIRECT_REST=0`). `=1`: the bad
//! record consumes the rest of the pass budget and following records run empty.

use std::sync::OnceLock;

/// The most single-record draws merged into one windowed prelude job.
pub(crate) const WINDOW_MOST: u32 = 64;

/// `XCL_INDIRECT_WINDOW=1`: windowed ahead-of-draw clamp hoisting for mesh records.
pub(crate) fn indirect_window() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_INDIRECT_WINDOW").is_some_and(|v| v == "1"))
}

/// `XCL_INDIRECT_BATCH`: unset → `1`; `0` / `1` / `2` as in Play-VIctory / CGBJ.
pub(crate) fn indirect_batch() -> u8 {
    static MODE: OnceLock<u8> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("XCL_INDIRECT_BATCH").as_deref() {
        Ok("0") => 0,
        Ok("2") => 2,
        _ => 1,
    })
}

/// `XCL_INDIRECT_PRELUDE=0`: inline clamps only (moved here from `bind` for one switch site).
pub(crate) fn indirect_prelude() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_INDIRECT_PRELUDE").map_or(true, |v| v.trim() != "0"))
}

/// Whether prelude hoisting is allowed at all for this batch mode.
pub(crate) fn prelude_hoist_allowed() -> bool {
    indirect_prelude() && indirect_batch() != 0
}

/// After an indirect or direct compute dispatch: with batch `2`, argument clamps may still hoist
/// until a barrier or indirect draw whose shaders may touch argument memory.
pub(crate) fn compute_dispatch_blocks_prelude() -> bool {
    indirect_batch() != 2
}

/// `XCL_INDIRECT_REST=1`: one oversized ExecuteIndirect record consumes the rest of its batch budget.
/// Unset / `0` (default): later records in the same batch that fit still run (CGBJ `VICTORY_INDIRECT_REST=0`).
pub(crate) fn indirect_rest() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_INDIRECT_REST").is_ok_and(|v| v.trim() == "1"))
}

/// HLSL `REST` define for batched indirect clamp shaders (see CGBJ `indirect/batch.rs`).
pub(crate) fn indirect_rest_hlsl() -> &'static str {
    if indirect_rest() { "true" } else { "false" }
}

/// Log once when batch/window env is active (smoke aid; no performance claims).
pub(crate) fn log_policy_once() {
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.set(()).is_err() {
        return;
    }
    let batch = indirect_batch();
    if batch == 2 {
        crate::xlog_info!("D3D12.X: indirect argument clamps use batch mode 2 (compute alone does not end the batch)");
    } else if batch == 0 {
        crate::xlog_info!("D3D12.X: indirect argument clamps are not batched in the prelude (XCL_INDIRECT_BATCH=0)");
    }
    if indirect_window() {
        crate::xlog_info!(
            "D3D12.X: consecutive single-record indirect mesh draws are clamped in windows of up to {WINDOW_MOST} (XCL_INDIRECT_WINDOW=1)"
        );
    }
    if indirect_rest() {
        crate::xlog_info!("D3D12.X: XCL_INDIRECT_REST=1 (oversized indirect record takes batch remainder)");
    }
    let _ = super::render_env::indirect_wide();
    let _ = super::render_env::indirect_card_budget();
    let _ = super::render_env::memory_card_mb();
}

/// An open window of single-record indirect draw guards being merged into one prelude job.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WindowOpen {
    pub buf_idx: usize,
    pub dst_off: u32,
    pub src_win: u32,
    pub src_off: u32,
    pub stride: u32,
    pub draw_offset: u32,
    pub count: u32,
}

impl WindowOpen {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_open(&self) -> bool {
        self.count != 0
    }

    /// Whether `src` at `off` continues this window (same buffer window, next slot by `stride`).
    pub fn continues(&self, src_win: u32, off: u32, stride: u32, draw_offset: u32) -> bool {
        let Some(next_off) = self.count.checked_mul(stride).and_then(|s| self.src_off.checked_add(s)) else {
            return false;
        };
        self.is_open()
            && self.stride == stride
            && self.draw_offset == draw_offset
            && self.src_win == src_win
            && off == next_off
            && self.count < WINDOW_MOST
    }

    pub fn open(&mut self, buf_idx: usize, dst_off: u32, src_win: u32, src_off: u32, stride: u32, draw_offset: u32) {
        *self = Self { buf_idx, dst_off, src_win, src_off, stride, draw_offset, count: 1 };
    }

    pub fn extend(&mut self) {
        if self.count < WINDOW_MOST {
            self.count += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_continues_by_stride() {
        let mut w = WindowOpen::default();
        w.open(0, 0, 1, 100, 32, 0);
        assert!(w.continues(1, 132, 32, 0));
        assert!(!w.continues(1, 164, 16, 0));
    }

    #[test]
    fn indirect_rest_default_off() {
        assert!(!indirect_rest());
        assert_eq!(indirect_rest_hlsl(), "false");
    }
}