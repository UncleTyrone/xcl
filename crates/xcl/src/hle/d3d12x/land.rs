//! Merge GPU readback bytes into title memory (`XCL_LAND=1`). Default off.
//!
//! Sparse UMA uses `note_sparse_landed` after readback; full shadow merge uses [`merge_range`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use super::watch;

/// 64 KiB guard block (matches watch / sparse tile).
pub(crate) const BLOCK: usize = 64 << 10;
const PAGE: usize = 0x1000;
const DWORD: usize = 4;

static STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
const S_PIECES: usize = 0;
const S_NEW_BYTES: usize = 1;
const S_DWORDS: usize = 2;
const S_BLOCKS: usize = 3;

fn count(i: usize, n: u64) {
    STATS[i].fetch_add(n, Ordering::Relaxed);
}

/// `XCL_LAND=1` (absent/off otherwise).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| super::port::land())
}

pub fn exact_back() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_EXACT_BACK").is_none_or(|v| v != "0"))
}

pub fn direct_writes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_DIRECT_WRITES").is_none_or(|v| v != "0"))
}

pub fn fill_title() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_FILL_TITLE").is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_LAND_BESIDE=1`: note landed blocks for the next refresh (no extra mapping yet).
pub fn beside() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| super::port::land_beside())
}

/// Worker count (`XCL_LAND_THREADS`, default ~cpus/4 clamped 1–4).
pub fn threads() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("XCL_LAND_THREADS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get() / 4).clamp(1, 4))
            .clamp(1, 16)
    })
}

/// Merge `src` over title `[dst, dst+len)` when bytes differ from `held` (last known card view).
/// Respects watch claims per 64 KiB block. Returns (new bytes, dwords stored).
pub unsafe fn merge_range(dst: usize, src: &[u8], held: &[u8], len: usize) -> (usize, usize) {
    let len = len.min(src.len()).min(held.len());
    if len == 0 {
        return (0, 0);
    }
    let lo_b = dst / BLOCK;
    let hi_b = dst.saturating_add(len.saturating_sub(1)) / BLOCK;
    let blocks: Vec<usize> = (lo_b..=hi_b).collect();
    let n_blocks = blocks.len();
    let _claims = watch::claim_blocks(&blocks);

    let apply_piece = |lo: usize, hi: usize| -> (usize, usize) {
        let mut nb = 0usize;
        let mut dw = 0usize;
        let mut i = lo;
        while i + DWORD <= hi {
            let cur = std::slice::from_raw_parts((dst + i) as *const u8, DWORD);
            let card = &src[i..i + DWORD];
            let snap = &held[i..i + DWORD];
            if card != snap {
                nb += DWORD;
                if cur != card {
                    std::ptr::copy_nonoverlapping(card.as_ptr(), (dst + i) as *mut u8, DWORD);
                    dw += 1;
                }
            }
            i += DWORD;
        }
        (nb, dw)
    };

    let (new_bytes, dwords) = if super::wide_back::enabled() && len > 256 << 10 {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let nb = AtomicUsize::new(0);
        let dw = AtomicUsize::new(0);
        super::wide_back::parallel_byte_jobs(len, 256 << 10, &|_, lo, n| {
            let (a, b) = apply_piece(lo, lo + n);
            nb.fetch_add(a, Ordering::Relaxed);
            dw.fetch_add(b, Ordering::Relaxed);
        });
        (nb.load(Ordering::Relaxed), dw.load(Ordering::Relaxed))
    } else {
        apply_piece(0, len)
    };

    if beside() {
        for &b in &blocks {
            watch::note_landed_block(b);
        }
    }
    count(S_NEW_BYTES, new_bytes as u64);
    count(S_DWORDS, dwords as u64);
    count(S_PIECES, 1);
    count(S_BLOCKS, n_blocks as u64);
    (new_bytes, dwords)
}

/// After sparse synchronous readback: journal landed 64 KiB blocks (beside path).
pub fn note_sparse_landed(ranges: &[(usize, usize)]) {
    if !enabled() || ranges.is_empty() {
        return;
    }
    count(S_PIECES, ranges.len() as u64);
  if beside() {
        for &(va, n) in ranges {
            if n == 0 {
                continue;
            }
            let lo = va / BLOCK;
            let hi = va.saturating_add(n.saturating_sub(1)) / BLOCK;
            for b in lo..=hi {
                watch::note_landed_block(b);
            }
        }
    }
}

/// Per-frame stats for the activity line.
pub fn log_stats(frames: f64) {
    if !enabled() || frames <= 0.0 {
        return;
    }
    let p = STATS[S_PIECES].swap(0, Ordering::Relaxed) as f64 / frames;
    if p == 0.0 {
        return;
    }
    let bytes = STATS[S_NEW_BYTES].swap(0, Ordering::Relaxed) as f64 / frames;
    let dw = STATS[S_DWORDS].swap(0, Ordering::Relaxed) as f64 / frames;
    crate::xlog_info!(
        "D3D12.X: land per frame (XCL_LAND=1, {} threads): {:.0} pieces, {:.0} new bytes, {:.0} dwords",
        threads(),
        p,
        bytes,
        dw
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_no_change_zero_dwords() {
        let mut mem = [0u8; 64];
        let src = [0u8; 64];
        let held = [0u8; 64];
        let (n, d) = unsafe { merge_range(mem.as_mut_ptr() as usize, &src, &held, 64) };
        assert_eq!(n, 0);
        assert_eq!(d, 0);
    }
}