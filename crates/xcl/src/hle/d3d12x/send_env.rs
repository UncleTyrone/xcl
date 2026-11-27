//! Submission send-buffer env latches (CGBJ parity; GPL-2 reimplementation).

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

/// Per-submission send buffer (`XCL_SEND_OWN`; on unless `0`).
pub fn send_own() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_SEND_OWN", "VICTORY_SEND_OWN").is_none_or(|v| v != "0");
        if env_os("XCL_SEND_OWN", "VICTORY_SEND_OWN").is_some() {
            log_once(
                "XCL_SEND_OWN",
                if on {
                    "per-submission send buffers"
                } else {
                    "shared send buffers"
                },
            );
        }
        on
    })
}

/// Place send memory in card BAR / upload heap (`XCL_SEND_BAR=1`; off).
pub fn send_bar() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_SEND_BAR", "VICTORY_SEND_BAR").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_SEND_BAR", "send buffers in card memory via BAR");
        }
        on
    })
}

/// Send only dirty 64 KiB pages (`XCL_SEND_CHANGED=1`; off unless set ≠ `0`).
pub fn send_changed() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_SEND_CHANGED", "VICTORY_SEND_CHANGED").is_some_and(|v| v != "0");
        if on {
            log_once("XCL_SEND_CHANGED", "send only dirty 64 KiB pages");
        }
        on
    })
}

/// Spare send-buffer pool cap in MiB (`XCL_SEND_SPARE_MB`; unset → 768 host / 256 BAR).
pub fn send_spare_mb() -> Option<u64> {
    static MB: OnceLock<Option<u64>> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_var("XCL_SEND_SPARE_MB", "VICTORY_SEND_SPARE_MB")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        if let Some(mb) = mb {
            log_once("XCL_SEND_SPARE_MB", &format!("{mb} MiB spare send pool"));
        }
        mb
    })
}

/// Spare pool bytes for current [`send_bar`] mode.
pub fn send_spare_bytes() -> u64 {
    match send_spare_mb() {
        Some(mb) => mb.saturating_mul(1 << 20),
        None if send_bar() => 256 << 20,
        None => 768 << 20,
    }
}

/// Touch latches when card-buffer / send path is entered.
pub fn on_send_path() {
    let _ = (
        send_own(),
        send_bar(),
        send_changed(),
        send_spare_bytes(),
        super::render_env::under_words(),
        super::render_env::upload_low(),
        super::render_env::wait_resident(),
        super::render_env::split_waits(),
    );
}
