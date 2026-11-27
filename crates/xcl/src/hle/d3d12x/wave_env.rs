//! Wave64 / mesh-shader env latches (CGBJ `VICTORY_WAVE_*` parity; GPL-2).

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

/// Opt-in deep skip form for wave64 (`XCL_WAVE_DEEP_SKIPS=1`; default off).
pub fn wave_deep_skips() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_WAVE_DEEP_SKIPS", "VICTORY_WAVE_DEEP_SKIPS").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_WAVE_DEEP_SKIPS", "=1 — deep skip / scalar-follow form");
        }
        on
    })
}

/// Wave-local lowering policy (`XCL_WAVE_LOCAL`: unset/on, `0` off, `idiom` strict).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveLocal {
    Off,
    On,
    Idiom,
}

pub fn wave_local() -> WaveLocal {
    static MODE: OnceLock<WaveLocal> = OnceLock::new();
    *MODE.get_or_init(|| {
        let mode = match env_var("XCL_WAVE_LOCAL", "VICTORY_WAVE_LOCAL").as_deref().map(str::trim) {
            Ok("0") => WaveLocal::Off,
            Ok("idiom") => WaveLocal::Idiom,
            _ => WaveLocal::On,
        };
        if env_os("XCL_WAVE_LOCAL", "VICTORY_WAVE_LOCAL").is_some() {
            log_once("XCL_WAVE_LOCAL", &format!("{mode:?}"));
        }
        mode
    })
}

/// Wave OR own-bits when not every lane runs (`XCL_WAVE_OWN_BITS`; default on).
pub fn wave_own_bits() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_WAVE_OWN_BITS", "VICTORY_WAVE_OWN_BITS").is_none_or(|v| v != "0");
        if env_os("XCL_WAVE_OWN_BITS", "VICTORY_WAVE_OWN_BITS").is_some_and(|v| v == "0") {
            log_once("XCL_WAVE_OWN_BITS", "=0 — refuse partial-lane wave OR");
        }
        on
    })
}

/// Log why a program keeps 64-lane waves (`XCL_WAVE_WHY` set).
pub fn wave_why() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_WAVE_WHY", "VICTORY_WAVE_WHY").is_some();
        if on {
            log_once("XCL_WAVE_WHY", "explain wave64 retention");
        }
        on
    })
}

/// Opt-in wave64 barrier form (`XCL_WAVE_WIDE=1`; default off — known TDR risk).
pub fn wave_wide() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_WAVE_WIDE", "VICTORY_WAVE_WIDE").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_WAVE_WIDE", "=1 — wave64 barrier form (opt-in)");
        } else if env_os("XCL_WAVE_WIDE", "VICTORY_WAVE_WIDE").is_some_and(|v| v == "0") {
            log_once("XCL_WAVE_WIDE", "=0 — wave64 barrier form off");
        }
        on
    })
}

/// Comma-separated shader digests that force wave-wide (`XCL_WAVE_WIDE_ONLY`).
pub fn wave_wide_only() -> &'static [String] {
    static LIST: OnceLock<Vec<String>> = OnceLock::new();
    LIST.get_or_init(|| {
        let raw = env_var("XCL_WAVE_WIDE_ONLY", "VICTORY_WAVE_WIDE_ONLY").unwrap_or_default();
        let v: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !v.is_empty() {
            log_once("XCL_WAVE_WIDE_ONLY", &format!("{} digest prefix(es)", v.len()));
        }
        v
    })
    .as_slice()
}

/// Touch latches from shader translate / cache paths.
pub fn on_shader_path() {
    let _ = (
        wave_deep_skips(),
        wave_local(),
        wave_own_bits(),
        wave_why(),
        wave_wide(),
        wave_wide_only().len(),
    );
}
