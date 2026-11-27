//! Shader/recomp env latches (`XCL_F16_NEAREST`, `XCL_FIRSTLANE_OWN`, …).

use std::path::PathBuf;
use std::sync::OnceLock;

fn log_once(name: &'static str, detail: &str) {
    static SEEN: OnceLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let set = SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    if let Ok(mut g) = set.lock() {
        if g.insert(name) {
            crate::xlog_info!("shader: {name} — {detail}");
        }
    }
}

/// `XCL_F16_NEAREST=1`: f32→f16 rounds to nearest even (console parity).
pub fn f16_nearest() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_F16_NEAREST").is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_FIRSTLANE_OWN=1`: V_READFIRSTLANE uses the host wave's lane 0.
pub fn firstlane_own() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_FIRSTLANE_OWN").is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_GROUP_SYNC=1` logged for group-barrier strict tests (default off).
pub fn group_sync_strict() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_GROUP_SYNC").is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_HAIR=1` enables hair-strand pipeline buckets (default off).
pub fn hair_pipelines() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_HAIR").is_some())
}

/// Console defrag compute hash (`VICTORY_DEFRAG=0` leaves it untranslated).
pub const DEFRAG_SHADER_HASH: u64 = 0x02384a60fda6f2d0;

/// Default per-lane translated loop exit (`XCL_SHADER_LOOP_LIMIT` overrides).
pub const DEFAULT_LOOP_LIMIT: u32 = 1 << 20;

/// `XCL_DEFRAG=0` with `VICTORY_DEFRAG=0`: skip translating the defrag compute shader.
pub fn defrag_off() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let off = std::env::var_os("XCL_DEFRAG")
            .or(std::env::var_os("VICTORY_DEFRAG"))
            .is_some_and(|v| v == "0");
        if off {
            crate::xlog_info!("shader: XCL_DEFRAG=0 — defrag compute left untranslated");
        }
        off
    })
}

fn parse_u32(value: Option<&str>, unset: u32) -> u32 {
    let parsed = value.map(str::trim).and_then(|v| match v.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => v.parse().ok(),
    });
    parsed.filter(|&n| n >= 1).unwrap_or(unset)
}

/// Per-lane loop iteration cap (`XCL_SHADER_LOOP_LIMIT`; default 2^20).
pub fn shader_loop_limit() -> u32 {
    static LIMIT: OnceLock<u32> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        let raw = std::env::var("XCL_SHADER_LOOP_LIMIT")
            .or_else(|_| std::env::var("VICTORY_SHADER_LOOP_LIMIT"))
            .ok();
        let limit = parse_u32(raw.as_deref(), DEFAULT_LOOP_LIMIT);
        if raw.is_some() {
            log_once("XCL_SHADER_LOOP_LIMIT", &format!("{limit} iterations per lane"));
        }
        limit
    })
}

/// Append census lines to this path when set (`XCL_SHADER_CENSUS=<file>`).
pub fn shader_census_path() -> Option<&'static PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::var_os("XCL_SHADER_CENSUS")
            .or_else(|| std::env::var_os("VICTORY_SHADER_CENSUS"))
            .map(PathBuf::from);
        if let Some(ref p) = path {
            log_once("XCL_SHADER_CENSUS", &format!("append emit lines to {}", p.display()));
        }
        path
    })
    .as_ref()
}

/// Directory for dumping title + translated shaders (`XCL_SHADER_SAVE=<dir>`).
pub fn shader_save_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::var_os("XCL_SHADER_SAVE")
            .or_else(|| std::env::var_os("VICTORY_SHADER_SAVE"))
            .map(PathBuf::from);
        if let Some(ref d) = dir {
            log_once("XCL_SHADER_SAVE", &format!("dump shaders under {}", d.display()));
        }
        dir
    })
    .as_ref()
}

/// Translator source fingerprint (`XCL_SHADER_SOURCE_HASH`; falls back to build id).
pub fn shader_source_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| {
        if let Ok(v) = std::env::var("XCL_SHADER_SOURCE_HASH").or_else(|_| std::env::var("VICTORY_SHADER_SOURCE_HASH")) {
            let s = v.trim().to_string();
            if !s.is_empty() {
                log_once("XCL_SHADER_SOURCE_HASH", &format!("override {s}"));
                return s;
            }
        }
        env!("XCL_TRANSLATOR_SOURCE_ID").to_string()
    })
    .as_str()
}

/// Comma-separated switch names folded into the translation fingerprint (`XCL_SHADER_SWITCHES`).
pub fn shader_switches() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| {
        if let Ok(v) = std::env::var("XCL_SHADER_SWITCHES").or_else(|_| std::env::var("VICTORY_SHADER_SWITCHES")) {
            let s = v.trim().to_string();
            if !s.is_empty() {
                log_once("XCL_SHADER_SWITCHES", &format!("override list ({})", s.split(',').count()));
                return s;
            }
        }
        "XCL_F16_NEAREST,XCL_FIRSTLANE_OWN,XCL_GROUP_SYNC,XCL_SHADER_LOOP_LIMIT,XCL_SHADER_OPT,XCL_DEFRAG,XCL_HAIR"
            .to_string()
    })
    .as_str()
}

/// 16-digit shader hashes whose pipelines are dropped (`XCL_SKIP_PIPELINES`).
pub fn skip_pipelines() -> &'static [String] {
    static HASHES: OnceLock<Vec<String>> = OnceLock::new();
    HASHES
        .get_or_init(|| {
            let value = std::env::var("XCL_SKIP_PIPELINES")
                .or_else(|_| std::env::var("VICTORY_SKIP_PIPELINES"))
                .unwrap_or_default();
            let hashes: Vec<String> = value
                .split(',')
                .map(str::trim)
                .filter(|h| !h.is_empty() && *h != "none")
                .filter_map(|hash| {
                    if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                        Some(hash.to_ascii_lowercase())
                    } else {
                        crate::xlog_warn!("shader: XCL_SKIP_PIPELINES expects 16-digit hex hashes (got {hash})");
                        None
                    }
                })
                .collect();
            if !hashes.is_empty() {
                log_once("XCL_SKIP_PIPELINES", &format!("{} shader hash(es) skipped", hashes.len()));
            }
            hashes
        })
        .as_slice()
}

/// True when `hash` (16 hex digits) is listed in [`skip_pipelines`].
pub fn is_skipped_shader_hash(hash: &str) -> bool {
    let h = hash.trim().to_ascii_lowercase();
    skip_pipelines().iter().any(|s| s == &h)
}

/// Bytes that should key the shader disk cache for translator env switches.
pub fn translation_fingerprint() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(shader_source_hash().as_bytes());
    out.push(0);
    for name in shader_switches().split(',').filter(|n| !n.is_empty()) {
        out.extend_from_slice(name.as_bytes());
        out.push(b'=');
        if let Some(value) = std::env::var_os(name) {
            out.extend_from_slice(value.as_encoded_bytes());
        }
        out.push(0);
    }
    out.extend_from_slice(&shader_loop_limit().to_le_bytes());
    out
}