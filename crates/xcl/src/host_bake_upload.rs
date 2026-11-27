//! One-shot material / decoration bake uploads use looser commit gates and sync list submit.

use std::cell::Cell;

thread_local! {
    static BAKING: Cell<u32> = const { Cell::new(0) };
}

/// `XCL_BAKE_SYNC=1` routes first-upload batches through synchronous queue submit when residency is on.
pub fn sync_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::host_residency::on() && std::env::var("XCL_BAKE_SYNC").is_ok_and(|v| !(v.trim().is_empty() || v.trim() == "0")))
}

pub fn active() -> bool {
    BAKING.get() > 0
}

/// Mark the current thread as running a bake pass (lower commit thresholds in [`crate::host_residency`]).
pub fn with_bake<R>(f: impl FnOnce() -> R) -> R {
    BAKING.set(BAKING.get().saturating_add(1));
    let out = f();
    BAKING.set(BAKING.get().saturating_sub(1));
    out
}

/// A whole-texture first upload is treated like a bake for admission when sync is on.
pub fn treat_first_as_bake(first: bool) -> bool {
    sync_on() && first
}