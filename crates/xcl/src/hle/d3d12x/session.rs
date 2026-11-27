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

/// `XCL_CARD_BUFFERS_MIN_KB` (default 64); used when MIN_MB is 0.
pub fn card_buffers_min_kb() -> u64 {
    static KB: OnceLock<u64> = OnceLock::new();
    *KB.get_or_init(|| {
        std::env::var("XCL_CARD_BUFFERS_MIN_KB")
            .or_else(|_| std::env::var("VICTORY_CARD_BUFFERS_MIN_KB"))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(64)
    })
}

pub fn card_buffers_min_bytes() -> u64 {
    let mb = card_buffers_min_mb();
    if mb > 0 { mb << 20 } else { card_buffers_min_kb() << 10 }
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
    super::send_env::on_send_path();
    let mut budget = card_buffers_budget_bytes();
    if let Some(mb) = super::send_env::send_spare_mb() {
        budget = budget.min(mb.saturating_mul(1 << 20).max(1 << 20));
    }
    let _ = (
        super::send_env::send_own(),
        super::send_env::send_bar(),
        super::send_env::send_changed(),
        super::send_env::send_spare_bytes(),
    );
    let tenth = budget / 10;
    let cap = budget.saturating_sub(tenth);
    let room = CARD_BYTES.load(Ordering::Relaxed).saturating_add(bytes) <= cap;
    if !room {
        maybe_card_accounting();
    }
    room
}

/// `XCL_CARD_ACCOUNTING=1`: log when card copy budget is tight (first hit, then every 5 s).
pub fn card_accounting() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("XCL_CARD_ACCOUNTING").is_ok_and(|v| v.trim() == "1")
            || std::env::var("VICTORY_CARD_ACCOUNTING").is_ok_and(|v| v.trim() == "1");
        if on {
            crate::xlog_info!("D3D12.X: XCL_CARD_ACCOUNTING=1");
        }
        on
    })
}

fn maybe_card_accounting() {
    if !card_accounting() {
        return;
    }
    static NEXT_MS: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_millis() as u64)
        .max(1);
    let next = NEXT_MS.load(Ordering::Relaxed);
    if next != 0 && now < next {
        return;
    }
    if NEXT_MS
        .compare_exchange(next, now + 5000, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    crate::xlog_info!(
        "D3D12.X: card copy budget tight ({} B tracked, cap {} B; XCL_CARD_ACCOUNTING)",
        CARD_BYTES.load(Ordering::Relaxed),
        card_buffers_budget_bytes() * 9 / 10
    );
}

/// `XCL_CARD_ALL=1`: every title buffer copy gets PC and card sides (default off).
pub fn card_all() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("XCL_CARD_ALL").is_ok_and(|v| v.trim() == "1")
            || std::env::var("VICTORY_CARD_ALL").is_ok_and(|v| v.trim() == "1");
        if on {
            crate::xlog_info!("D3D12.X: XCL_CARD_ALL=1 — dual-sided card copies for all buffers");
        }
        on
    })
}

/// `XCL_CARD_HASH_LEN=0`: sparse compare covers the whole card buffer (default: title length rounded to 16 B).
pub fn card_hash_own_len() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("XCL_CARD_HASH_LEN")
            .or(std::env::var_os("VICTORY_CARD_HASH_LEN"))
            .is_none_or(|v| v != "0")
    })
}

pub fn card_hash_compare_bytes(title_len: u64, card_size: u64) -> u64 {
    if !card_hash_own_len() {
        if std::env::var_os("XCL_CARD_HASH_LEN").is_some_and(|v| v == "0") {
            static LOG: OnceLock<()> = OnceLock::new();
            LOG.get_or_init(|| crate::xlog_info!("D3D12.X: XCL_CARD_HASH_LEN=0 — compare whole card buffer"));
        }
        return card_size;
    }
    title_len.next_multiple_of(16).min(card_size)
}

/// `XCL_CARD_SPANS=1`: stamp GPU time around submissions (diagnostic latch; default off).
pub fn card_spans() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("XCL_CARD_SPANS").is_ok_and(|v| v.trim() == "1")
            || std::env::var("VICTORY_CARD_SPANS").is_ok_and(|v| v.trim() == "1");
        if on {
            crate::xlog_info!("D3D12.X: XCL_CARD_SPANS=1");
        }
        on
    })
}

/// `XCL_MEMORY_CARD_MB` budget for title-memory-by-address card sides (default 4096).
pub fn memory_card_budget_bytes() -> u64 {
    static B: OnceLock<u64> = OnceLock::new();
    *B.get_or_init(|| {
        let mb = env_mb(&["XCL_MEMORY_CARD_MB", "VICTORY_MEMORY_CARD_MB"], 4096);
        crate::xlog_info!("D3D12.X: XCL_MEMORY_CARD_MB={mb}");
        mb << 20
    })
}

static MEMORY_CARD_USED: AtomicU64 = AtomicU64::new(0);

/// Charge `len` against the memory-card budget; returns whether it fits.
pub fn memory_card_room(len: u64) -> bool {
    let most = memory_card_budget_bytes();
    MEMORY_CARD_USED.fetch_add(len, Ordering::Relaxed) + len <= most
}