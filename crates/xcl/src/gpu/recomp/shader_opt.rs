//! Opt-in IR shrink before DXIL is written (`XCL_SHADER_OPT`, legacy `VICTORY_SHADER_OPT`). Default off.
//!
//! `1` / `on` enables the direct-translator cleanup passes in [`crate::gpu::dxil::module::opt`].
//! A decimal or `0x` bitmask is reserved for future per-pass control (non-zero enables shrink).
//! See cool_fox `VICTORY_SHADER_OPT` (~43% IR reduction when all module passes run).

use std::sync::OnceLock;

fn raw() -> Option<String> {
    ["XCL_SHADER_OPT", "VICTORY_SHADER_OPT"]
        .into_iter()
        .find_map(|n| std::env::var(n).ok().map(|v| v.trim().to_string()))
}

/// Whether IR/DXIL shrink passes should run (unset or `0` → false).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let Some(v) = raw() else {
            return false;
        };
        let s = v.as_str();
        if s == "0" || s.eq_ignore_ascii_case("off") {
            return false;
        }
        if s == "1" || s.eq_ignore_ascii_case("on") {
            crate::xlog_info!("[SHADER] XCL_SHADER_OPT=1 — DXIL IR shrink passes on");
            return true;
        }
        let n = s
            .strip_prefix("0x")
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .or_else(|| s.parse().ok());
        let on = n.is_some_and(|n| n != 0);
        if on {
            crate::xlog_info!("[SHADER] XCL_SHADER_OPT={s} — DXIL IR shrink passes on");
        }
        on
    })
}

/// Gating for [`crate::gpu::dxil::module::Module::optimize`]: opt-in in release, always on in unit tests.
pub fn dxil_shrink() -> bool {
    enabled() || cfg!(test)
}

/// Cache identity suffix when shrink is active (any non-off value).
pub fn cache_tag() -> Option<&'static str> {
    static TAG: OnceLock<Option<String>> = OnceLock::new();
    TAG.get_or_init(|| raw().filter(|v| {
        let s = v.as_str();
        s != "0" && !s.eq_ignore_ascii_case("off")
    }))
    .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_is_off() {
        if raw().is_none() {
            assert!(!enabled());
        }
    }
}