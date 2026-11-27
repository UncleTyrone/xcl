//! Pipeline / join-list boot latches (`XCL_PIPELINE_*`, `XCL_JOIN_LISTS`). CGBJ parity (GPL-2).

use std::sync::OnceLock;

fn log_once(name: &'static str, detail: &str) {
    static SEEN: OnceLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let set = SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    if let Ok(mut g) = set.lock() {
        if g.insert(name) {
            crate::xlog_info!("D3D12.X: {name} — {detail}");
        }
    }
}

fn env_os(xcl: &str, victory: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(xcl).or_else(|| std::env::var_os(victory))
}

fn env_str(xcl: &str, victory: &str) -> Option<String> {
    std::env::var(xcl).ok().or_else(|| std::env::var(victory).ok())
}

/// `XCL_PIPELINE_BOOK=1`: keep title-created pipeline keys for warm/preload (default off).
pub fn pipeline_book() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_PIPELINE_BOOK", "VICTORY_PIPELINE_BOOK").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_PIPELINE_BOOK", "1 — record title pipelines for warm/preload");
        }
        on
    })
}

/// `XCL_PIPELINE_LOWEST=0`: pipeline workers run below-normal (default lowest).
pub fn pipeline_lowest() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_PIPELINE_LOWEST", "VICTORY_PIPELINE_LOWEST").is_none_or(|v| v != "0");
        if env_os("XCL_PIPELINE_LOWEST", "VICTORY_PIPELINE_LOWEST").is_some_and(|v| v == "0") {
            log_once("XCL_PIPELINE_LOWEST", "=0 — workers below-normal, not lowest");
        }
        on
    })
}

/// `XCL_PIPELINE_SLOW=0`: no cap on concurrent slow early pipelines (default: ~workers/4).
pub fn pipeline_slow_limit(workers: usize) -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if env_os("XCL_PIPELINE_SLOW", "VICTORY_PIPELINE_SLOW").is_some_and(|v| v == "0") {
            log_once("XCL_PIPELINE_SLOW", "=0 — no slow-pipeline concurrency cap");
            usize::MAX
        } else {
            (workers / 4).max(1)
        }
    })
}

/// Shared per-frame wait budget across threads (default on; `XCL_PIPELINE_WAIT_SHARED=0` per-thread).
pub fn pipeline_wait_shared() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_PIPELINE_WAIT_SHARED", "VICTORY_PIPELINE_WAIT_SHARED").is_none_or(|v| v != "0");
        if env_os("XCL_PIPELINE_WAIT_SHARED", "VICTORY_PIPELINE_WAIT_SHARED").is_some_and(|v| v == "0") {
            log_once("XCL_PIPELINE_WAIT_SHARED", "=0 — per-thread pipeline wait budgets");
        }
        on
    })
}

/// `XCL_PIPELINE_WORKERS=<n>`: PSO worker count (1–64); else ~⅔ of logical CPUs clamped 2–16.
pub fn pipeline_workers() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        let processors = std::thread::available_parallelism().map_or(2, |n| n.get());
        let n = match env_str("XCL_PIPELINE_WORKERS", "VICTORY_PIPELINE_WORKERS")
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            Some(asked) => asked.clamp(1, 64),
            None => (processors * 2 / 3).clamp(2, 16),
        };
        if env_str("XCL_PIPELINE_WORKERS", "VICTORY_PIPELINE_WORKERS").is_some() {
            log_once("XCL_PIPELINE_WORKERS", &format!("{n} pipeline workers"));
        }
        n
    })
}

/// Max lists joined in one thin translate batch (`XCL_JOIN_LISTS`, default 48).
pub fn join_lists() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_JOIN_LISTS", "VICTORY_JOIN_LISTS")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(48)
            .max(1);
        if env_str("XCL_JOIN_LISTS", "VICTORY_JOIN_LISTS").is_some() {
            log_once("XCL_JOIN_LISTS", &format!("join up to {n} lists per thin batch"));
        }
        n
    })
}

/// Touch latches once so log-once runs early on the PSO path.
pub fn note_boot() {
    let _ = (
        pipeline_book(),
        pipeline_lowest(),
        pipeline_wait_shared(),
        pipeline_workers(),
        pipeline_slow_limit(pipeline_workers()),
        join_lists(),
    );
}
