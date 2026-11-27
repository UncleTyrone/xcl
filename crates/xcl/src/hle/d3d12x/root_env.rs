//! Root-descriptor / RT resolve env latches (CGBJ parity; GPL-2 reimplementation).

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

/// Prefer one-view tables for root SRVs (bounded); `XCL_ROOT_SRV_DIRECT=1` uses root descriptors.
pub fn root_srv_direct() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let direct = env_os("XCL_ROOT_SRV_DIRECT", "VICTORY_ROOT_SRV_DIRECT").is_some();
        if direct {
            log_once("XCL_ROOT_SRV_DIRECT", "bind root SRVs as root descriptors");
        }
        direct
    })
}

/// Whether root SRVs use bounded one-view tables (default; opposite of [`root_srv_direct`]).
pub fn bounded_root_srvs() -> bool {
    !root_srv_direct()
}

/// Cap root SRV refresh length in MiB (`XCL_ROOT_VIEW_MB`; unset → no cap).
pub fn root_view_mb() -> Option<u64> {
    static MB: OnceLock<Option<u64>> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_var("XCL_ROOT_VIEW_MB", "VICTORY_ROOT_VIEW_MB")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        if let Some(mb) = mb {
            log_once("XCL_ROOT_VIEW_MB", &format!("{mb} MiB root SRV view cap"));
        }
        mb
    })
}

pub fn root_view_bytes() -> u64 {
    root_view_mb().map_or(u64::MAX, |mb| mb << 20)
}

/// Cap root UAV refresh / copy-back in MiB (`XCL_ROOT_UAV_MB`; unset → no cap).
pub fn root_uav_mb() -> Option<u64> {
    static MB: OnceLock<Option<u64>> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_var("XCL_ROOT_UAV_MB", "VICTORY_ROOT_UAV_MB")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        if let Some(mb) = mb {
            log_once("XCL_ROOT_UAV_MB", &format!("{mb} MiB root UAV view cap"));
        }
        mb
    })
}

pub fn root_uav_bytes() -> u64 {
    root_uav_mb().map_or(u64::MAX, |mb| mb << 20)
}

/// Resolve RT export names during cold PSO compile (`XCL_RT_RESOLVE=1`; default skip).
pub fn rt_resolve() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_RT_RESOLVE", "VICTORY_RT_RESOLVE").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_RT_RESOLVE", "resolve RT exports during cold compile");
        }
        on
    })
}

/// Touch from root-descriptor binds.
pub fn on_root_bind() {
    let _ = (root_srv_direct(), root_view_bytes(), root_uav_bytes(), rt_resolve());
}
