//! Dynamic descriptor recycle and host-memory policy (CGBJ Phase 12 subset).
//!
//! `XCL_RESIDENCY=1` enables deferring dynamic descriptor ranges until the submit fence completes.

use std::sync::OnceLock;

/// `XCL_RESIDENCY=1` enables fence-deferred dynamic descriptor recycle (off by default).
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_RESIDENCY").is_ok_and(|v| !(v.trim().is_empty() || v.trim() == "0")))
}

/// Hold `MIN_DYNAMIC..` ranges until the queue submit fence passes (`XCL_RESIDENCY=1`).
pub fn defer_dynamic_slots() -> bool {
    on()
}