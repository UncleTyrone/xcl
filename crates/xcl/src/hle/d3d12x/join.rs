//! Parallel fan-out for thin submit translation (`XCL_JOIN_PARTS`, default 8 when thin is on).

use std::sync::Mutex;

/// Worker count when [`super::thin::enabled`] is on (from `XCL_JOIN_PARTS`, default 8).
pub fn parts() -> usize {
    super::thin::join_parts()
}

/// Run `f(i)` for `i in 0..n` across at most [`parts()`] threads (scoped; joins before return).
pub fn parallel_for(n: usize, f: &(dyn Fn(usize) + Sync)) {
    if n == 0 {
        return;
    }
    let workers = parts().max(1).min(n);
    if workers == 1 {
        for i in 0..n {
            f(i);
        }
        return;
    }
    let chunk = (n + workers - 1) / workers;
    std::thread::scope(|scope| {
        for w in 0..workers {
            let lo = w * chunk;
            if lo >= n {
                break;
            }
            let hi = (lo + chunk).min(n);
            scope.spawn(move || {
                for i in lo..hi {
                    f(i);
                }
            });
        }
    });
}

/// Parallel `write[i] = f(i)` for `i in 0..n`.
pub fn parallel_collect<T: Send, F: Fn(usize) -> T + Sync>(n: usize, f: F) -> Vec<T> {
    if n == 0 {
        return Vec::new();
    }
    let workers = parts().max(1).min(n);
    if workers == 1 {
        return (0..n).map(|i| f(i)).collect();
    }
    let slots: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    parallel_for(n, &|i| {
        let v = f(i);
        slots.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(v);
    });
    slots
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .map(|o| o.expect("parallel_collect index"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_for_covers_each_index_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let n = 17;
        let hits = AtomicUsize::new(0);
        parallel_for(n, &|_| {
            hits.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(hits.load(Ordering::Relaxed), n);
    }
}