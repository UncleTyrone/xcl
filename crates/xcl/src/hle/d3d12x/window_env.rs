//! Window / shadow-grid env latches (CGBJ `VICTORY_WINDOW_*` parity; GPL-2).

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

fn env_var(xcl: &str, victory: &str) -> Result<String, std::env::VarError> {
    std::env::var(xcl).or_else(|_| std::env::var(victory))
}

/// Idle ms before reclaiming unused windows (`XCL_WINDOW_IDLE_MS`; unset → free-memory policy).
pub fn window_idle_ms() -> Option<u64> {
    static MS: OnceLock<Option<u64>> = OnceLock::new();
    *MS.get_or_init(|| {
        let ms = env_var("XCL_WINDOW_IDLE_MS", "VICTORY_WINDOW_IDLE_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        if let Some(ms) = ms {
            log_once("XCL_WINDOW_IDLE_MS", &format!("{ms} ms idle before window reclaim"));
        }
        ms
    })
}

/// Effective idle ms floored at 2 s when env override is set.
pub fn window_idle_effective(computed: u64) -> u64 {
    window_idle_ms().map_or(computed, |ms| ms.max(2_000))
}

fn window_grid_bytes(kb: Option<u64>) -> u64 {
    const DEFAULT: u64 = 4 << 20;
    let Some(kb) = kb else {
        return DEFAULT;
    };
    let bytes = kb.saturating_mul(1024);
    if bytes < 64 * 1024 {
        return DEFAULT;
    }
    // Nearest power of two at or below ask, at least 64 KiB, at most 4 MiB.
    let mut g = 64u64 << 10;
    while g < bytes && g < DEFAULT {
        let next = g.saturating_mul(2);
        if next > bytes || next > DEFAULT {
            break;
        }
        g = next;
    }
    g
}

/// Shadow window grid in bytes (`XCL_WINDOW_KB`; default 4 MiB).
pub fn window_kb_bytes() -> u64 {
    static B: OnceLock<u64> = OnceLock::new();
    *B.get_or_init(|| {
        let kb = env_var("XCL_WINDOW_KB", "VICTORY_WINDOW_KB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok());
        let bytes = window_grid_bytes(kb);
        if kb.is_some() {
            log_once("XCL_WINDOW_KB", &format!("{} KiB grid ({} bytes)", bytes >> 10, bytes));
        }
        bytes
    })
}

fn parse_window_max(raw: &str) -> Option<(u32, u32)> {
    let lower = raw.to_ascii_lowercase();
    let (w, h) = lower.split_once('x')?;
    let w: u32 = w.trim().parse().ok()?;
    let h: u32 = h.trim().parse().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// Cap for the present window (`XCL_WINDOW_MAX=WIDTHxHEIGHT`; unset = no cap).
pub fn window_max() -> Option<(u32, u32)> {
    static LIMIT: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        let raw = env_os("XCL_WINDOW_MAX", "VICTORY_WINDOW_MAX")?;
        let Some(s) = raw.to_str() else {
            log_once("XCL_WINDOW_MAX", "ignored (non-UTF8)");
            return None;
        };
        match parse_window_max(s) {
            Some(wh) => {
                log_once("XCL_WINDOW_MAX", &format!("{}x{}", wh.0, wh.1));
                Some(wh)
            }
            None => {
                log_once("XCL_WINDOW_MAX", "ignored; expected positive WIDTHxHEIGHT");
                None
            }
        }
    })
}

/// Clamp `(w,h)` into [`window_max`] when set, preserving aspect.
pub fn clamp_window_size(w: u32, h: u32) -> (u32, u32) {
    let Some((mw, mh)) = window_max() else {
        return (w, h);
    };
    if w <= mw && h <= mh {
        return (w, h);
    }
    let sx = mw as f64 / w as f64;
    let sy = mh as f64 / h as f64;
    let s = sx.min(sy);
    (
        (((w as f64) * s).round() as u32).max(1),
        (((h as f64) * s).round() as u32).max(1),
    )
}

/// Allow shorter window tails (`XCL_WINDOW_TAILS=1`; default off).
pub fn window_tails() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_WINDOW_TAILS", "VICTORY_WINDOW_TAILS").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_WINDOW_TAILS", "=1 — shorter window tails");
        }
        on
    })
}

/// Touch latches from present / shadow paths.
pub fn on_window_path() {
    let _ = (window_idle_ms(), window_kb_bytes(), window_max(), window_tails());
}
