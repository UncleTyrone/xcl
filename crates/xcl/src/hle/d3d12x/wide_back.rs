//! Parallel compare/merge jobs (`XCL_WIDE_BACK=1`). Default off.

use std::sync::OnceLock;

pub fn enabled() -> bool {
    super::port::wide_back()
}

static LOGGED: OnceLock<()> = OnceLock::new();

fn log_once() {
    LOGGED.get_or_init(|| {
        crate::xlog_info!("D3D12.X: wide_back (XCL_WIDE_BACK=1, uses XCL_JOIN_PARTS workers)");
    });
}

pub fn parallel_for(n: usize, f: &(dyn Fn(usize) + Sync)) {
    if n == 0 {
        return;
    }
    if !enabled() || n == 1 {
        for i in 0..n {
            f(i);
        }
        return;
    }
    log_once();
    super::join::parallel_for(n, f);
}

pub fn parallel_byte_jobs(total: usize, target: usize, f: &(dyn Fn(usize, usize, usize) + Sync)) {
    if total == 0 {
        return;
    }
    let target = target.max(64 << 10);
    let jobs = (total + target - 1) / target;
    parallel_for(jobs, &|j| {
        let lo = j * target;
        if lo >= total {
            return;
        }
        let len = target.min(total - lo);
        f(j, lo, len);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn parallel_for_sequential_when_off() {
        let n = AtomicUsize::new(0);
        parallel_for(5, &|_| {
            n.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(n.load(Ordering::Relaxed), 5);
    }
}