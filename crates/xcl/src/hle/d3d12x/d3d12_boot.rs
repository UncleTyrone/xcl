//! D3D12 host boot knobs (`XCL_D3D12_WARP`, `XCL_D3D12_GBV`, `XCL_D3D12_BACKGROUND`).

use std::sync::OnceLock;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;

fn env_is_1(names: &[&str]) -> bool {
    names.iter().any(|n| std::env::var(*n).is_ok_and(|v| v.trim() == "1"))
}

/// `XCL_D3D12_WARP=1`: create the host device on the software adapter (slow; no TDR reset).
pub fn force_warp() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_is_1(&["XCL_D3D12_WARP", "VICTORY_D3D12_WARP"]);
        if on {
            crate::xlog_info!("D3D12.X: XCL_D3D12_WARP=1 — host device on WARP");
        }
        on
    })
}

fn gbv_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| env_is_1(&["XCL_D3D12_GBV", "VICTORY_D3D12_GBV"]))
}

fn background_off() -> bool {
    std::env::var_os("XCL_D3D12_BACKGROUND")
        .or(std::env::var_os("VICTORY_D3D12_BACKGROUND"))
        .is_some_and(|v| v == "0")
}

pub fn apply_device_boot(device: &ID3D12Device, debug_requested: bool) {
    if debug_requested || gbv_on() {
        let _ = crate::gpu::agility::enable_debug_layer();
    }
    if gbv_on() {
        enable_gbv();
    }
    if background_off() {
        disable_background(device);
    }
}

fn enable_gbv() {
    static LOGGED: OnceLock<()> = OnceLock::new();
    if crate::gpu::agility::enable_debug_layer().is_err() {
        return;
    }
    let Ok(debug) = crate::gpu::agility::configuration_interface::<ID3D12Debug>(&CLSID_D3D12Debug) else {
        return;
    };
  // SAFETY: valid debug interface from agility loader.
    unsafe {
        if let Ok(d1) = debug.cast::<ID3D12Debug1>() {
            d1.SetEnableGPUBasedValidation(true);
        }
        if let Ok(d2) = debug.cast::<ID3D12Debug2>() {
            d2.SetGPUBasedValidationFlags(D3D12_GPU_BASED_VALIDATION_FLAGS_DISABLE_STATE_TRACKING);
        }
    }
    LOGGED.get_or_init(|| {
        crate::xlog_info!("D3D12.X: XCL_D3D12_GBV=1 — GPU-based validation on (state tracking off)");
    });
}

fn disable_background(device: &ID3D12Device) {
  // SAFETY: valid device from init.
    unsafe {
        if let Ok(dev6) = device.cast::<ID3D12Device6>() {
            let set = dev6.SetBackgroundProcessingMode(
                D3D12_BACKGROUND_PROCESSING_MODE_DISABLE_BACKGROUND_WORK,
                D3D12_MEASUREMENTS_ACTION_KEEP_ALL,
                None,
                None,
            );
            crate::xlog_info!(
                "D3D12.X: XCL_D3D12_BACKGROUND=0 — driver background shader work off (ok={})",
                set.is_ok()
            );
        }
    }
}