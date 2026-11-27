//! Thin list recording + parallel translate-at-submit (`XCL_THIN=1`, `XCL_JOIN_PARTS`).

use std::ffi::c_void;
use std::sync::OnceLock;

/// `XCL_THIN=1`.
pub fn enabled() -> bool {
    super::port::thin()
}

/// Parallel translate workers when thin is on (`XCL_JOIN_PARTS`, else `XCL_THIN_THREADS`, default 8).
pub fn join_parts() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        let n = std::env::var("XCL_JOIN_PARTS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| super::render_env::thin_threads());
        n.clamp(1, 64)
    })
}

/// Max lists in one thin join batch (`XCL_JOIN_LISTS`, default 48).
pub fn join_lists() -> usize {
    super::pipeline_env::join_lists()
}

/// When thin: do not build or submit prelude lists from KickoffX (join at ExecuteCommandLists).
pub fn defer_kickoff_prelude() -> bool {
    enabled()
}

/// Record deferred dispatches for one title list (submit lane).
pub unsafe fn translate_one(wrapper: *mut c_void) {
    super::list_x::record_deferred(wrapper);
}

/// Expand one submitted list into PC segments (with preludes). Title-list wrappers only.
pub unsafe fn push_one(list: *mut c_void, out: &mut Vec<*mut c_void>) -> bool {
    if let Some(pc) = super::console_list::pc_list(list) {
        out.push(pc);
        return true;
    }
    if super::title_list::push_segments(list, out) {
        return true;
    }
    out.push(list);
    true
}

/// Per-list segment vectors joined in list order before `ExecuteCommandLists`.
pub unsafe fn map_lists(lists: &[*mut c_void]) -> Vec<*mut c_void> {
    let n = lists.len();
    if n == 0 {
        return Vec::new();
    }
    for &l in lists {
        if super::console_list::pc_list(l).is_none() {
            translate_one(l);
        }
    }
    if !enabled() || n == 1 {
        let mut mapped = Vec::new();
        for &l in lists {
            push_one(l, &mut mapped);
        }
        return mapped;
    }
    let cap = join_lists().min(n);
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        crate::xlog_info!(
            "D3D12.X: thin translate join ({n} lists, {} workers, JOIN_LISTS={cap})",
            super::join::parts()
        );
    });
    let mut mapped = Vec::new();
    let mut offset = 0usize;
    while offset < n {
        let end = (offset + cap).min(n);
        let batch = &lists[offset..end];
        let bn = batch.len();
        let titles: Vec<usize> = batch.iter().map(|&p| p as usize).collect();
        let chunks: Vec<Vec<usize>> = super::join::parallel_collect(bn, |i| {
            let mut segs = Vec::new();
            // SAFETY: list pointers stay live until ExecuteCommandLists returns; each index is distinct.
            let list = titles[i] as *mut c_void;
            unsafe { push_one(list, &mut segs) };
            segs.into_iter().map(|p| p as usize).collect()
        });
        for segs in chunks {
            mapped.extend(segs.into_iter().map(|p| p as *mut c_void));
        }
        offset = end;
    }
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_parts_default_when_unset() {
        assert!((1..=64).contains(&join_parts()));
    }
}