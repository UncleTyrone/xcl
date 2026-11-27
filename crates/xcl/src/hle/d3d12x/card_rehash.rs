//! Join dirty sparse chunk runs across 64 KiB pieces (`XCL_CARD_REHASH_RUNS=1`). Default off.
//!
//! Port of cool_fox `joined_piece_runs` for the sparse upload path (fewer compare/upload passes).

use std::sync::OnceLock;

pub const PIECE: usize = 64 << 10;

fn join_runs_impl(mut runs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    runs.sort_unstable_by_key(|r| r.0);
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(runs.len());
    for (addr, len) in runs {
        if len == 0 {
            continue;
        }
        let end = addr.saturating_add(len);
        let first = addr / PIECE;
        match out.last_mut() {
            Some((a, l)) => {
                let prev_end = a.saturating_add(*l);
                let prev_last = prev_end.saturating_sub(1) / PIECE;
                if first <= prev_last.saturating_add(1) {
                    let new_end = end.max(prev_end);
                    *l = new_end.saturating_sub(*a);
                } else {
                    out.push((addr, len));
                }
            }
            None => out.push((addr, len)),
        }
    }
    out
}

/// Merge `(addr, len)` runs whose 64 KiB piece ranges touch or overlap (sorted by address).
pub fn join_runs(runs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if !super::port::card_rehash_runs() || runs.is_empty() {
        return runs;
    }
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        crate::xlog_info!("D3D12.X: XCL_CARD_REHASH_RUNS=1 — join sparse chunk compare/upload runs");
    });
    join_runs_impl(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touches_merge_adjacent_pieces() {
        let a = PIECE - 64;
        let merged = join_runs_impl(vec![(a, 64), (PIECE, 128)]);
        assert_eq!(merged, vec![(a, PIECE + 128)]);
    }
}