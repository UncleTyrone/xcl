//! BVH / DXR env latches (CGBJ parity; GPL-2 reimplementation).

use std::sync::OnceLock;

/// `XCL_ACTIVATE_RAYS=0` disables DXR activation paths (default on unless `0`).
pub fn activate_rays() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        if std::env::var_os("XCL_RAYTRACING").is_some_and(|v| {
            let s = v.to_string_lossy();
            s.trim() == "0" || s.eq_ignore_ascii_case("off")
        }) {
            return false;
        }
        let on = std::env::var_os("XCL_ACTIVATE_RAYS").is_none_or(|v| v != "0");
        if std::env::var_os("XCL_ACTIVATE_RAYS").is_some() {
            crate::xlog_info!("D3D12.X: XCL_ACTIVATE_RAYS={}", if on { "1 (rays on)" } else { "0" });
        }
        on
    })
}

/// `XCL_BVH_BOUNDS=0` turns off bounds patching in BVH builder shaders (default on).
pub fn bvh_bounds() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_BVH_BOUNDS").is_none_or(|v| v != "0"))
}

/// `XCL_BVH_FENCES=0` drops fence inserts in BVH emit (default on).
pub fn bvh_fences() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_BVH_FENCES").is_none_or(|v| v != "0"))
}

/// `XCL_BVH_QUARANTINE=1` keeps builder stage quarantine (default off).
pub fn bvh_quarantine() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_BVH_QUARANTINE").is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_BVH_TRACE=1` logs BVH size requests.
pub fn bvh_trace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_BVH_TRACE").is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_BVH_SIZES_WAIT=1` waits for landed sizes before proceeding (default off).
pub fn bvh_sizes_wait() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("XCL_BVH_SIZES_WAIT").is_ok_and(|v| v.trim() == "1"))
}