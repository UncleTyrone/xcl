//! Texture refresh ask pacing (`XCL_ASK_*`). CGBJ parity; hooks on title refresh.

use std::sync::OnceLock;
use std::time::Duration;

fn log_once(name: &'static str, detail: &str) {
    static SEEN: OnceLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let set = SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    if let Ok(mut g) = set.lock() {
        if g.insert(name) {
            crate::xlog_info!("D3D12.X: {name} — {detail}");
        }
    }
}

/// `XCL_ASK_EARLY=<ms>`: slice interval for early asks between submission rounds (off unless set).
pub fn ask_early_ms() -> Option<f64> {
    static MS: OnceLock<Option<f64>> = OnceLock::new();
    *MS.get_or_init(|| {
        let raw = std::env::var("XCL_ASK_EARLY").or_else(|_| std::env::var("VICTORY_ASK_EARLY"));
        let Ok(v) = raw else { return None };
        let ms = v.trim().parse::<f64>().unwrap_or(2.0);
        if ms <= 0.0 {
            return None;
        }
        log_once("XCL_ASK_EARLY", &format!("{ms} ms between early ask rounds"));
        Some(ms)
    })
}

pub fn ask_early_every() -> Option<Duration> {
    ask_early_ms().and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok().filter(|d| !d.is_zero()))
}

/// `XCL_ASK_NORESET=1`: journal only pages found written since the last ask (default off).
pub fn ask_no_reset() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_on_1("XCL_ASK_NORESET") || env_on_1("VICTORY_ASK_NORESET");
        if on {
            log_once("XCL_ASK_NORESET", "do not reset write-watch during interleaved asks");
        }
        on
    })
}

/// `XCL_ASK_SPREAD=1`: spread asks across submission threads (default off).
pub fn ask_spread() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_on_1("XCL_ASK_SPREAD") || env_on_1("VICTORY_ASK_SPREAD");
        if on {
            log_once("XCL_ASK_SPREAD", "spread asks across threads");
        }
        on
    })
}

/// `XCL_ASK_ONCE=0`: per-range asks; default once per refresh span.
pub fn ask_once_per_refresh() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let once = std::env::var_os("XCL_ASK_ONCE")
            .or(std::env::var_os("VICTORY_ASK_ONCE"))
            .is_none_or(|v| v != "0");
        if std::env::var_os("XCL_ASK_ONCE").is_some_and(|v| v == "0")
            || std::env::var_os("VICTORY_ASK_ONCE").is_some_and(|v| v == "0")
        {
            log_once("XCL_ASK_ONCE", "=0 — ask each refreshed range separately");
        }
        once
    })
}

fn env_on_1(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v.trim() == "1")
}

/// Called at the start of each title texture refresh when any ask knob is active.
pub fn on_refresh_start() {
    if ask_early_every().is_none() && !ask_no_reset() && !ask_spread() && ask_once_per_refresh() {
        return;
    }
    static SUMMARY: OnceLock<()> = OnceLock::new();
    SUMMARY.get_or_init(|| {
        crate::xlog_info!(
            "D3D12.X: ask path (XCL_ASK_EARLY={:?}, XCL_ASK_NORESET={}, XCL_ASK_SPREAD={}, XCL_ASK_ONCE={})",
            ask_early_ms(),
            ask_no_reset(),
            ask_spread(),
            ask_once_per_refresh(),
        );
    });
    maybe_start_early_thread();
}

fn maybe_start_early_thread() {
    let Some(every) = ask_early_every() else { return };
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new().name("xcl-ask-early".into()).spawn(move || loop {
            std::thread::sleep(every);
            super::watch::tick_early_ask();
        });
    });
}