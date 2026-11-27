//! Host-memory admission for texture prefetch and reusable upload storage (CGBJ Phase 12).
//!
//! Implemented in [`crate::host_residency`] so `gpu::texture` can gate staging growth without a
//! `gpu` ↔ `hle` module cycle.

pub use crate::host_residency::{coarse_ms, defer_dynamic_slots, on, refresh};
pub(crate) use crate::host_residency::{
    admit_texture_pass, allocation_error_hresult, allow_staging_growth, critical_commit, headroom,
    limited_host_ram, live_staging_bytes, note_staging_deferred, note_staging_growth_failed, prefetch_limit,
    pressured, set_live_staging_bytes, take_staging_deferred, upload_refused,
};