//! Session VRAM / card-copy budgets (`XCL_CARD_*`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static DEDICATED_VRAM_MB: OnceLock<u64> = OnceLock::new();

pub fn set_dedicated_vram_mb(mb: u64) {
    let _ = DEDICATED_VRAM_MB.set(mb.max(1));
}

fn env_mb(names: &[&'static str], default_mb: u64) -> u64 {
    for n in names {
        if let Ok(v) = std::env::var(n) {
            if let Ok(mb) = v.trim().parse::<u64>() {
                if mb > 0 {
                    return mb;
                }
            }
        }
    }
    default_mb
}

fn scale_from_16gb(reference_mb: u64) -> u64 {
    let vram = DEDICATED_VRAM_MB.get().copied().unwrap_or(16 * 1024);
    let scaled = reference_mb.saturating_mul(vram) / (16 * 1024);
    scaled.max(reference_mb / 2)
}

/// Card buffer copies enabled (`XCL_CARD_BUFFERS`, default on; `=0` off).
pub fn card_buffers_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("XCL_CARD_BUFFERS").is_none_or(|v| v != "0");
        if std::env::var_os("XCL_CARD_BUFFERS").is_some() {
            crate::xlog_info!("D3D12.X: XCL_CARD_BUFFERS={}", if on { "1" } else { "0" });
        }
        on
    })
}

/// `XCL_CARD_BUFFERS_MB` (default 2048 at 16 GB VRAM, scaled).
pub fn card_buffers_budget_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_mb(&["XCL_CARD_BUFFERS_MB", "VICTORY_CARD_BUFFERS_MB"], scale_from_16gb(2048));
        crate::xlog_info!("D3D12.X: card buffer copy budget {mb} MiB (XCL_CARD_BUFFERS_MB)");
        mb
    })
}

pub fn card_buffers_budget_bytes() -> u64 {
    card_buffers_budget_mb() << 20
}

/// `XCL_CARD_BUFFERS_MIN_MB` minimum card-copy buffer size (default 0 = use KiB latch).
pub fn card_buffers_min_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| {
        env_mb(&["XCL_CARD_BUFFERS_MIN_MB", "VICTORY_CARD_BUFFERS_MIN_MB"], 0)
    })
}

/// `XCL_CARD_WINDOWS_MB` extra UMA windows outside card memory (default 0).
pub fn card_windows_bytes() -> u64 {
    static B: OnceLock<u64> = OnceLock::new();
    *B.get_or_init(|| {
        let mb = env_mb(&["XCL_CARD_WINDOWS_MB", "VICTORY_CARD_WINDOWS_MB"], 0);
        if mb > 0 {
            crate::xlog_info!("D3D12.X: XCL_CARD_WINDOWS_MB={mb}");
        }
        mb << 20
    })
}

/// `XCL_CARD_ROOM_FOR_BUFFERS` (default on unless `=0`).
pub fn card_room_for_buffers() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_CARD_ROOM_FOR_BUFFERS").is_none_or(|v| v != "0"))
}

/// `XCL_CARD_ROOM_FOR_TARGETS_MB` headroom for render targets (default 3072 MiB ref).
pub fn card_room_for_targets_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| env_mb(&["XCL_CARD_ROOM_FOR_TARGETS_MB", "VICTORY_CARD_ROOM_FOR_TARGETS_MB"], 3072))
}

static CARD_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn note_card_bytes(bytes: u64) {
    if card_buffers_enabled() {
        CARD_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }
}

pub fn card_buffers_room(bytes: u64) -> bool {
    if !card_buffers_enabled() {
        return false;
    }
    let budget = card_buffers_budget_bytes();
    let tenth = budget / 10;
    let cap = budget.saturating_sub(tenth);
    CARD_BYTES.load(Ordering::Relaxed).saturating_add(bytes) <= cap
}