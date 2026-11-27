//! Indirect ExecuteIndirect policy fragments shared with amd-dev / nvidia-dev (CGBJ Phase 14).
//!
//! `XCL_INDIRECT_REST` — unset/`0` (default): an oversized indirect record in a batched pass takes
//! no budget and later records that fit still run (CGBJ `VICTORY_INDIRECT_REST=0`). `=1`: the bad
//! record consumes the rest of the pass budget and following records run empty (amd prelude HLSL).

use std::sync::OnceLock;

/// `XCL_INDIRECT_REST=1`: one oversized ExecuteIndirect record consumes the rest of its batch budget.
/// Unset / `0` (default): later records in the same batch that fit still run (CGBJ `VICTORY_INDIRECT_REST=0`).
pub(crate) fn indirect_rest() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_INDIRECT_REST").is_ok_and(|v| v.trim() == "1"))
}

/// HLSL `REST` define for batched indirect clamp shaders (see CGBJ `indirect/batch.rs`).
pub(crate) fn indirect_rest_hlsl() -> &'static str {
    if indirect_rest() { "true" } else { "false" }
}

/// HLSL prefix for indirect clamp shaders (`REST`, CGBJ batch budget policy).
pub(crate) fn indirect_clamp_shader_source(body: &str) -> String {
    log_policy_once();
    format!("#define REST {}\n{body}", indirect_rest_hlsl())
}

pub(crate) fn log_policy_once() {
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.set(()).is_err() {
        return;
    }
    if indirect_rest() {
        crate::xlog_info!("D3D12.X: XCL_INDIRECT_REST=1 (oversized indirect record takes batch remainder)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indirect_rest_default_off() {
        assert!(!indirect_rest());
    }

    #[test]
    fn indirect_clamp_shader_prepends_rest() {
        let src = indirect_clamp_shader_source("void main() {}");
        assert!(src.starts_with("#define REST false\n"));
    }
}