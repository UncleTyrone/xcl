//! Texture / refresh / staging env latches (CGBJ parity; GPL-2 reimplementation).

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

/// Delay before an unmapped texture slot is cleared (`XCL_TEXTURE_AWAY_MS`; default 3000).
pub fn texture_away_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| {
        let ms = env_var("XCL_TEXTURE_AWAY_MS", "VICTORY_TEXTURE_AWAY_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(3000);
        if env_os("XCL_TEXTURE_AWAY_MS", "VICTORY_TEXTURE_AWAY_MS").is_some() {
            log_once("XCL_TEXTURE_AWAY_MS", &format!("{ms} ms before unmapped texture slots clear"));
        }
        ms
    })
}

/// Triple budget while texture upload backlog is large (`XCL_TEXTURE_CATCHUP`; on unless `0`).
pub fn texture_catchup() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_TEXTURE_CATCHUP", "VICTORY_TEXTURE_CATCHUP").is_none_or(|v| v != "0");
        if env_os("XCL_TEXTURE_CATCHUP", "VICTORY_TEXTURE_CATCHUP").is_some() {
            log_once(
                "XCL_TEXTURE_CATCHUP",
                if on {
                    "triple upload budget while backlog drains"
                } else {
                    "plain upload budget only"
                },
            );
        }
        on
    })
}

/// Force kernel copy for fully mapped title texture reads (`XCL_TEXTURE_COPY=1`; off).
pub fn texture_copy() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_TEXTURE_COPY", "VICTORY_TEXTURE_COPY").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_TEXTURE_COPY", "read mapped textures through kernel copy");
        }
        on
    })
}

/// Fill staging from whatever pages are present when unlocked (`XCL_TEXTURE_FILL`; on unless `0`).
pub fn texture_fill() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_TEXTURE_FILL", "VICTORY_TEXTURE_FILL").is_none_or(|v| v != "0");
        if env_os("XCL_TEXTURE_FILL", "VICTORY_TEXTURE_FILL").is_some_and(|v| v == "0") {
            log_once("XCL_TEXTURE_FILL", "=0 — leave unread staging bytes as made");
        }
        on
    })
}

/// Parallel ahead refresh of shader-written copies (`XCL_REFRESH_AHEAD`; on unless `0`, needs wide+light).
pub fn refresh_ahead() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_REFRESH_AHEAD", "VICTORY_REFRESH_AHEAD").is_none_or(|v| v != "0");
        if env_os("XCL_REFRESH_AHEAD", "VICTORY_REFRESH_AHEAD").is_some() {
            log_once(
                "XCL_REFRESH_AHEAD",
                if on {
                    "refresh shader-written copies ahead of steps"
                } else {
                    "refresh each copy before its step"
                },
            );
        }
        on
    })
}

/// Triple-stale host-copy diagnostic (`XCL_REFRESH_CHECK=1`; off).
pub fn refresh_check() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_REFRESH_CHECK", "VICTORY_REFRESH_CHECK").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_REFRESH_CHECK", "alarm when host copy stays stale after refresh");
        }
        on
    })
}

/// Refresh entire submission front ahead of steps (`XCL_REFRESH_FRONT=1`; off unless set).
pub fn refresh_front() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_REFRESH_FRONT", "VICTORY_REFRESH_FRONT").is_some_and(|v| {
            let s = v.to_string_lossy();
            s != "0" && !s.is_empty()
        });
        if on {
            log_once("XCL_REFRESH_FRONT", "refresh whole submission ahead of steps");
        }
        on
    })
}

/// Keep updating existing host textures when VRAM is full (`XCL_REUPLOAD_WHEN_FULL`; on unless `0`).
pub fn reupload_when_full() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_REUPLOAD_WHEN_FULL", "VICTORY_REUPLOAD_WHEN_FULL").is_none_or(|v| v != "0");
        if env_os("XCL_REUPLOAD_WHEN_FULL", "VICTORY_REUPLOAD_WHEN_FULL").is_some() {
            log_once(
                "XCL_REUPLOAD_WHEN_FULL",
                if on {
                    "refresh existing textures when card is full"
                } else {
                    "skip reuploads when card is full"
                },
            );
        }
        on
    })
}

/// Staging idle reclaim override in ms (`XCL_STAGING_IDLE_MS`; unset → free-memory policy).
pub fn staging_idle_ms() -> Option<u64> {
    static MS: OnceLock<Option<u64>> = OnceLock::new();
    *MS.get_or_init(|| {
        let ms = env_var("XCL_STAGING_IDLE_MS", "VICTORY_STAGING_IDLE_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        if let Some(ms) = ms {
            log_once("XCL_STAGING_IDLE_MS", &format!("{ms} ms idle before staging reclaim"));
        }
        ms
    })
}

/// Effective idle ms: env override floored at 2 s, else `computed`.
pub fn idle_ms(computed: u64) -> u64 {
    staging_idle_ms().map_or(computed, |ms| ms.max(2_000))
}

/// Stale view policy (`XCL_STALE_TEXTURES`: unset=`start`, `0`=never, `overlap`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleRule {
    Never,
    Start,
    Overlap,
}

pub fn stale_textures() -> StaleRule {
    static RULE: OnceLock<StaleRule> = OnceLock::new();
    *RULE.get_or_init(|| {
        let rule = match env_var("XCL_STALE_TEXTURES", "VICTORY_STALE_TEXTURES").as_deref().map(str::trim) {
            Ok("0") => StaleRule::Never,
            Ok("overlap") => StaleRule::Overlap,
            _ => StaleRule::Start,
        };
        if env_os("XCL_STALE_TEXTURES", "VICTORY_STALE_TEXTURES").is_some() {
            log_once("XCL_STALE_TEXTURES", &format!("{rule:?}"));
        }
        rule
    })
}

/// Keep table-read slots in the index-picked queue (`XCL_TABLED_TEXTURES=1`); default drops them.
pub fn tabled_textures_keep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let keep = env_os("XCL_TABLED_TEXTURES", "VICTORY_TABLED_TEXTURES").is_some_and(|v| v == "1");
        if keep {
            log_once("XCL_TABLED_TEXTURES", "=1 — table-read slots stay in index queue");
        }
        keep
    })
}

/// Refresh dirty 4 KiB parts instead of whole pages (`XCL_TAKE_PARTS`; on unless `0`).
pub fn take_parts() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_TAKE_PARTS", "VICTORY_TAKE_PARTS").is_none_or(|v| v != "0");
        if env_os("XCL_TAKE_PARTS", "VICTORY_TAKE_PARTS").is_some_and(|v| v == "0") {
            log_once("XCL_TAKE_PARTS", "=0 — refresh whole pages");
        }
        on
    })
}

/// Skip hash when write-watch says unchanged (`XCL_TEXTURE_GUARD`; default on).
pub fn texture_guard() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_TEXTURE_GUARD", "VICTORY_TEXTURE_GUARD").is_none_or(|v| v != "0");
        if env_os("XCL_TEXTURE_GUARD", "VICTORY_TEXTURE_GUARD").is_some_and(|v| v == "0") {
            log_once("XCL_TEXTURE_GUARD", "=0 — hash every check");
        }
        on
    })
}

/// Prefer hot textures every frame early on (`XCL_TEXTURE_HOT`; default on).
pub fn texture_hot() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_TEXTURE_HOT", "VICTORY_TEXTURE_HOT").is_none_or(|v| v != "0");
        if env_os("XCL_TEXTURE_HOT", "VICTORY_TEXTURE_HOT").is_some_and(|v| v == "0") {
            log_once("XCL_TEXTURE_HOT", "=0 — every eighth frame only");
        }
        on
    })
}

/// Idle-texture keep policy (`XCL_TEXTURE_KEEP`: unset adaptive, `0`/`1` older rules).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureKeep {
    Adaptive,
    Off,
    Long,
}

pub fn texture_keep() -> TextureKeep {
    static KEEP: OnceLock<TextureKeep> = OnceLock::new();
    *KEEP.get_or_init(|| {
        let keep = match env_var("XCL_TEXTURE_KEEP", "VICTORY_TEXTURE_KEEP").as_deref().map(str::trim) {
            Ok("0") => TextureKeep::Off,
            Ok("1") => TextureKeep::Long,
            _ => TextureKeep::Adaptive,
        };
        if env_os("XCL_TEXTURE_KEEP", "VICTORY_TEXTURE_KEEP").is_some() {
            log_once("XCL_TEXTURE_KEEP", &format!("{keep:?}"));
        }
        keep
    })
}

/// Whether keep is not explicitly `0` (host idle-ms path).
pub fn texture_keep_enabled() -> bool {
    !matches!(texture_keep(), TextureKeep::Off)
}

/// Whether sweep uses adaptive free-room keep (unset; not `0`/`1`).
pub fn texture_keep_adaptive() -> bool {
    matches!(texture_keep(), TextureKeep::Adaptive)
}

/// First upload skips write-watch / guards (`XCL_TEXTURE_ONCE`; default on).
pub fn texture_once() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_TEXTURE_ONCE", "VICTORY_TEXTURE_ONCE").is_none_or(|v| v != "0");
        if env_os("XCL_TEXTURE_ONCE", "VICTORY_TEXTURE_ONCE").is_some_and(|v| v == "0") {
            log_once("XCL_TEXTURE_ONCE", "=0 — ask write-watch on first upload");
        }
        on
    })
}

/// Percent of card budget left free for textures (`XCL_TEXTURE_ROOM_PERCENT`; default 90, clamp 50..=97).
pub fn texture_room_percent() -> u64 {
    static P: OnceLock<u64> = OnceLock::new();
    *P.get_or_init(|| {
        let p = env_var("XCL_TEXTURE_ROOM_PERCENT", "VICTORY_TEXTURE_ROOM_PERCENT")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map_or(90, |percent| percent.clamp(50, 97));
        if env_os("XCL_TEXTURE_ROOM_PERCENT", "VICTORY_TEXTURE_ROOM_PERCENT").is_some() {
            log_once("XCL_TEXTURE_ROOM_PERCENT", &format!("{p}% room kept free"));
        }
        p
    })
}

/// Skip reupload when host copy bytes match (`XCL_TEXTURE_SAME`; default on).
pub fn texture_same() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_TEXTURE_SAME", "VICTORY_TEXTURE_SAME").is_none_or(|v| v != "0");
        if env_os("XCL_TEXTURE_SAME", "VICTORY_TEXTURE_SAME").is_some_and(|v| v == "0") {
            log_once("XCL_TEXTURE_SAME", "=0 — always reupload");
        }
        on
    })
}

/// Per-frame staging budget override in MiB (`XCL_TEXTURE_STAGE_MB`; unset → heuristic).
pub fn texture_stage_mb() -> Option<u64> {
    static MB: OnceLock<Option<u64>> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_var("XCL_TEXTURE_STAGE_MB", "VICTORY_TEXTURE_STAGE_MB")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        if let Some(mb) = mb {
            log_once("XCL_TEXTURE_STAGE_MB", &format!("{mb} MiB per-frame staging"));
        }
        mb
    })
}

pub fn texture_stage_bytes() -> Option<u64> {
    texture_stage_mb().map(|mb| mb.saturating_mul(1 << 20))
}

/// Staging time slice in ms (`XCL_TEXTURE_STAGE_MS`; default 250).
pub fn texture_stage_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| {
        let ms = env_var("XCL_TEXTURE_STAGE_MS", "VICTORY_TEXTURE_STAGE_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(250);
        if env_os("XCL_TEXTURE_STAGE_MS", "VICTORY_TEXTURE_STAGE_MS").is_some() {
            log_once("XCL_TEXTURE_STAGE_MS", &format!("{ms} ms staging slice"));
        }
        ms
    })
}

/// Idle texture reclaim (`XCL_TEXTURE_SWEEP`: unset batched, `0` off, `whole` all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureSweep {
    Off,
    Batched,
    Whole,
}

pub fn texture_sweep() -> TextureSweep {
    static MODE: OnceLock<TextureSweep> = OnceLock::new();
    *MODE.get_or_init(|| {
        let mode = match env_var("XCL_TEXTURE_SWEEP", "VICTORY_TEXTURE_SWEEP").as_deref().map(str::trim) {
            Ok("0") => TextureSweep::Off,
            Ok("whole") => TextureSweep::Whole,
            _ => TextureSweep::Batched,
        };
        if env_os("XCL_TEXTURE_SWEEP", "VICTORY_TEXTURE_SWEEP").is_some() {
            log_once("XCL_TEXTURE_SWEEP", &format!("{mode:?}"));
        }
        mode
    })
}

pub fn texture_sweep_on() -> bool {
    !matches!(texture_sweep(), TextureSweep::Off)
}

/// Touch latches once from the texture refresh path so log-once fires early.
pub fn on_refresh_start() {
    let _ = (
        texture_away_ms(),
        texture_catchup(),
        texture_copy(),
        texture_fill(),
        texture_guard(),
        texture_hot(),
        texture_keep(),
        texture_once(),
        texture_room_percent(),
        texture_same(),
        texture_stage_mb(),
        texture_stage_ms(),
        texture_sweep(),
        refresh_ahead(),
        refresh_check(),
        refresh_front(),
        reupload_when_full(),
        staging_idle_ms(),
        stale_textures(),
        tabled_textures_keep(),
        take_parts(),
    );
}
