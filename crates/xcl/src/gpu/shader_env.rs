//! Shader/recomp env latches (`XCL_F16_NEAREST`, `XCL_FIRSTLANE_OWN`, …).

use std::sync::OnceLock;

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