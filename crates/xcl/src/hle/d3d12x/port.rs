//! Opt-in renderer ports (`XCL_PORT_*`), read the way Belobug reads `VICTORY_PORT_*`.
//!
//! Absent means off. `1` means on. Each switch that is on is logged once the first time
//! something asks for it. Defaults on this tree stay off; device branches may flip a subset.

use std::sync::OnceLock;

/// `true` when `XCL_PORT_<name>` is set to `1` (any other value, including empty, is off).
fn env_on(name: &'static str) -> bool {
    std::env::var(name).is_ok_and(|v| v.trim() == "1")
}

fn once(name: &'static str, on: bool) -> bool {
    static LOGGED: OnceLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    if on {
        let set = LOGGED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
        if let Ok(mut g) = set.lock() {
            if g.insert(name) {
                crate::xlog_info!("D3D12.X: {name}=1");
            }
        }
    }
    on
}

fn latch(cell: &OnceLock<bool>, name: &'static str) -> bool {
    *cell.get_or_init(|| once(name, env_on(name)))
}

/// Port 1: in-place constants heap (`OpenExistingHeapFromAddress` on a second section view).
pub fn constants() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_PORT_CONSTANTS")
}

/// Kelvaroq 0038: CBV views the host copy (`XCL_DIRECT_CONSTANTS` / `VICTORY_DIRECT_CONSTANTS`).
pub fn direct_constants() -> bool {
    super::constants::direct_constants()
}

/// Port 2: title descriptor-store hooks (byte-checked sites in 1.0.2.1).
pub fn descriptors() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_PORT_DESCRIPTORS")
}

/// Port 5: raw CBV twins (requires [`constants`]).
pub fn cbv() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    let want = latch(&ON, "XCL_PORT_CBV");
    want && constants()
}

/// Port 6: shared guard lock / per-64 KiB claim.
pub fn guard() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_PORT_GUARD")
}

/// Port 6 plan beside the lock (on when guard is on unless `XCL_PORT_GUARD_PLAN=0`).
pub fn guard_plan() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        if !guard() {
            return false;
        }
        let on = std::env::var_os("XCL_PORT_GUARD_PLAN").is_none_or(|v| v != "0");
        once("XCL_PORT_GUARD_PLAN", on)
    })
}

/// Port 7: kick the descriptor journal at submit.
pub fn journal() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_PORT_JOURNAL")
}

/// Read ExecuteIndirect args after preceding barriers (constants-order fix).
pub fn constants_order() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_PORT_CONSTANTS_ORDER")
}

/// With [`constants_order`]: in-place indirect clamps never share a prelude batch (unset or `=0` off).
pub fn constants_order_alone() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_PORT_CONSTANTS_ORDER_ALONE").is_none_or(|v| v != "0"))
}

/// NVIDIA mesh amplification launch for indirect mesh draws.
pub fn mesh_launch() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    // Also accept the name used in Kelvaroq notes.
    *ON.get_or_init(|| {
        let on = env_on("XCL_MESH_LAUNCH") || env_on("XCL_PORT_MESH_LAUNCH");
        once("XCL_MESH_LAUNCH", on)
    })
}

/// Ambient stand-in after screen-probe resolve (scale string; empty/`0`/`off` = off).
pub fn ambient_scale() -> Option<f32> {
    static SCALE: OnceLock<Option<f32>> = OnceLock::new();
    *SCALE.get_or_init(|| {
        let raw = std::env::var("XCL_AMBIENT")
            .ok()
            .or_else(|| std::env::var("XCL_PORT_AMBIENT").ok())
            .unwrap_or_else(|| "0.6".to_owned());
        let s = raw.trim();
        if s.is_empty() || s == "0" || s.eq_ignore_ascii_case("off") {
            return None;
        }
        if let Some(rest) = s.strip_prefix("constant:") {
            return rest.parse().ok().filter(|v: &f32| *v > 0.0).map(|v| {
                once("XCL_AMBIENT", true);
                v
            });
        }
        s.parse::<f32>().ok().filter(|v| *v > 0.0).map(|v| {
            once("XCL_AMBIENT", true);
            v
        })
    })
}

/// Serial submit (one list + fence wait) until frames are short (`XCL_SUBMIT_SERIAL=0` to disable).
pub fn submit_serial() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = match std::env::var("XCL_SUBMIT_SERIAL") {
            Ok(v) => v.trim() == "1",
            Err(_) => true,
        };
        if on {
            crate::xlog_info!("D3D12.X: XCL_SUBMIT_SERIAL=1 (one command list and GPU wait per submit)");
        }
        on
    })
}

/// Thin list recording + translate workers.
pub fn thin() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_THIN")
}

/// Async present / submit frames-ahead cap (`XCL_FRAMES_AHEAD`, default 2 on nvidia-dev).
pub fn frames_ahead() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("XCL_FRAMES_AHEAD")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(2)
            .clamp(1, 4)
    })
}

/// Worker-thread submit (`XCL_ASYNC_SUBMIT=1`).
pub fn async_submit() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_ASYNC_SUBMIT")
}

/// Copy-queue mode: `0` off, `3` upload + readback copy queues (`XCL_QUEUE2=3` to opt in).
pub fn queue2() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        let v = std::env::var("XCL_QUEUE2")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let v = v.clamp(0, 3);
        if v > 0 {
            once("XCL_QUEUE2", true);
        }
        v
    })
}

pub fn queue2_active() -> bool {
    queue2() > 0
}

/// `rep movsb` title copies (`XCL_COPY_PLAIN=1`).
pub fn copy_plain() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_COPY_PLAIN")
}

/// One `GetWriteWatch` per graphics reservation per submit (`XCL_WATCH_RESERVATION=1`).
pub fn watch_reservation() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_WATCH_RESERVATION")
}

/// Target-format PSO variants on mismatch (default on unless `XCL_TARGET_VARIANTS=0`).
pub fn target_variants() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("XCL_TARGET_VARIANTS").is_none_or(|v| v != "0"))
}

/// CGBJ relay stage (`XCL_RELAY`: absent/0 off, `1` or `2`).
pub fn relay_stage() -> u32 {
    static STAGE: OnceLock<u32> = OnceLock::new();
    *STAGE.get_or_init(|| match std::env::var("XCL_RELAY").ok().as_deref().map(str::trim) {
        Some("1") => {
            once("XCL_RELAY", true);
            1
        }
        Some("2") => {
            once("XCL_RELAY", true);
            2
        }
        _ => 0,
    })
}

/// Fetch GPU-written ranges on the sparse copy queue (`XCL_RELAY_COPY=1`).
pub fn relay_copy() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_RELAY_COPY")
}

/// Parallel merge/compare during landing (`XCL_WIDE_BACK=1`).
pub fn wide_back() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_WIDE_BACK")
}

/// Multi-threaded title merge after GPU fetch (`XCL_LAND=1`).
pub fn land() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_LAND")
}

/// Land through guard-adjacent views (`XCL_LAND_BESIDE=1`).
pub fn land_beside() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_LAND_BESIDE")
}

/// GPU-side fill/copy prelude (`XCL_CARD_STEPS`: `1` or `2`, default `2` when set non-zero).
pub fn card_steps_stage() -> u32 {
    static STAGE: OnceLock<u32> = OnceLock::new();
    *STAGE.get_or_init(|| match std::env::var("XCL_CARD_STEPS").ok().as_deref().map(str::trim) {
        None | Some("") | Some("0") | Some("off") => 0,
        Some("1") => {
            once("XCL_CARD_STEPS", true);
            1
        }
        Some(_) => {
            once("XCL_CARD_STEPS", true);
            2
        }
    })
}

/// Coalesce adjacent title buffer copy steps (`XCL_JOIN_COPIES=1`).
pub fn join_copies() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    latch(&ON, "XCL_JOIN_COPIES")
}

/// Join compare re-hash passes across runs (`XCL_CARD_REHASH_RUNS=1`, legacy `VICTORY_CARD_REHASH_RUNS=1`).
pub fn card_rehash_runs() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_on("XCL_CARD_REHASH_RUNS")
            || std::env::var("VICTORY_CARD_REHASH_RUNS").is_ok_and(|v| v.trim() == "1");
        once("XCL_CARD_REHASH_RUNS", on);
        on
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_port_is_off() {
        // In the default test process none of these are set.
        assert!(!env_on("XCL_PORT_CONSTANTS_TEST_ABSENT_xyz"));
    }

    #[test]
    fn frames_ahead_clamps() {
        let n = frames_ahead();
        assert!((1..=4).contains(&n));
    }

    #[test]
  fn nvidia_default_frames_ahead_is_two() {
        if std::env::var_os("XCL_FRAMES_AHEAD").is_none() {
            assert_eq!(frames_ahead(), 2);
        }
    }

    #[test]
    fn target_variants_default_on() {
        assert!(target_variants());
    }

    #[test]
    fn queue2_default_off() {
        if std::env::var_os("XCL_QUEUE2").is_none() {
            assert_eq!(queue2(), 0);
            assert!(!queue2_active());
        }
    }
}
