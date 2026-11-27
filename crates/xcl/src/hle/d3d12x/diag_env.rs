//! Boot/render-adjacent diagnostic and host latches (CGBJ `VICTORY_*` parity; GPL-2).
//!
//! Capture, fault-book, shader-tool, host-wave, and related knobs. Defaults match CGBJ.

use std::path::PathBuf;
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

fn env_str(xcl: &str, victory: &str) -> Option<String> {
    std::env::var(xcl).ok().or_else(|| std::env::var(victory).ok())
}

fn default_on(xcl: &'static str, victory: &'static str, detail_on: &str) -> bool {
    let on = env_os(xcl, victory).is_none_or(|v| v != "0");
    if env_os(xcl, victory).is_some_and(|v| v == "0") {
        log_once(xcl, "=0 — off");
    } else if env_os(xcl, victory).is_some() {
        log_once(xcl, detail_on);
    }
    on
}

fn opt_in_1(xcl: &'static str, victory: &'static str, detail: &str) -> bool {
    let on = env_str(xcl, victory).is_some_and(|v| v.trim() == "1");
    if on {
        log_once(xcl, detail);
    }
    on
}

fn parse_u64_list(raw: &str) -> Vec<u64> {
    raw.split(',')
        .filter_map(|f| f.trim().parse().ok())
        .collect()
}

// —— CAPTURE_* ——

/// Explicit frame numbers to capture (`XCL_CAPTURE_FRAMES`, comma-separated).
pub fn capture_frames() -> &'static [u64] {
    static LIST: OnceLock<Vec<u64>> = OnceLock::new();
    LIST.get_or_init(|| {
        let list = env_str("XCL_CAPTURE_FRAMES", "VICTORY_CAPTURE_FRAMES").unwrap_or_default();
        let frames = parse_u64_list(&list);
        if !frames.is_empty() {
            log_once("XCL_CAPTURE_FRAMES", &format!("{} listed frame(s)", frames.len()));
        }
        frames
    })
    .as_slice()
}

/// Capture world-entry frames after the first heavy draw frame (`XCL_CAPTURE_WORLD` set).
pub fn capture_world() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_CAPTURE_WORLD", "VICTORY_CAPTURE_WORLD").is_some();
        if on {
            log_once("XCL_CAPTURE_WORLD", "capture frames after first heavy world frame");
        }
        on
    })
}

/// Extra offsets from the world-entry frame (`XCL_CAPTURE_WORLD_ALSO`, comma-separated).
pub fn capture_world_also() -> &'static [u64] {
    static LIST: OnceLock<Vec<u64>> = OnceLock::new();
    LIST.get_or_init(|| {
        let list = env_str("XCL_CAPTURE_WORLD_ALSO", "VICTORY_CAPTURE_WORLD_ALSO").unwrap_or_default();
        let frames = parse_u64_list(&list);
        if !frames.is_empty() {
            log_once("XCL_CAPTURE_WORLD_ALSO", &format!("{} extra world-relative frame(s)", frames.len()));
        }
        frames
    })
    .as_slice()
}

// —— host / helper / held / card / fetch ——

/// Auto-answer CLZERO asks (default on; `XCL_CLZERO_ASK=1` asks every time).
pub fn clzero_ask_auto() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let auto = env_os("XCL_CLZERO_ASK", "VICTORY_CLZERO_ASK").is_none_or(|v| v != "1");
        if env_os("XCL_CLZERO_ASK", "VICTORY_CLZERO_ASK").is_some_and(|v| v == "1") {
            log_once("XCL_CLZERO_ASK", "=1 — ask on every CLZERO");
        }
        auto
    })
}

/// Query committed ranges with VirtualQuery (`XCL_COMMITTED_QUERY=1`; default off).
pub fn committed_query() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_COMMITTED_QUERY", "VICTORY_COMMITTED_QUERY", "1 — VirtualQuery committed ranges"))
}

/// Ask before opportunistic host copies (`XCL_COPY_ASK=1`; default off).
pub fn copy_ask() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_COPY_ASK", "VICTORY_COPY_ASK", "1 — ask before opportunistic copies"))
}

/// Declare host views once per frame span (default on; `XCL_DECLARE_ONCE=0` every time).
pub fn declare_once() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_DECLARE_ONCE", "VICTORY_DECLARE_ONCE", "declare host views once per span"))
}

/// Log descriptor-lowering rationale (`XCL_DESC_WHY` set).
pub fn desc_why() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_DESC_WHY", "VICTORY_DESC_WHY").is_some();
        if on {
            log_once("XCL_DESC_WHY", "log descriptor lowering why");
        }
        on
    })
}

/// Leave Windows efficiency mode alone (`XCL_EFFICIENCY_MODE=1`; default: disable it).
pub fn efficiency_mode() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_EFFICIENCY_MODE", "VICTORY_EFFICIENCY_MODE").is_some_and(|v| v == "1");
        if on {
            log_once("XCL_EFFICIENCY_MODE", "1 — leave process power throttling to the system");
        }
        on
    })
}

/// Check fetched pieces against flight end state (`XCL_FETCH_CHECK=1`; default off).
pub fn fetch_check() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_FETCH_CHECK", "VICTORY_FETCH_CHECK", "1 — check fetched pieces per frame"))
}

/// Full-card texture pass holds back slots needing a new host copy (default on).
pub fn full_card_pass() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        default_on(
            "XCL_FULL_CARD_PASS",
            "VICTORY_FULL_CARD_PASS",
            "hold back slots that need a new host copy on a full card",
        )
    })
}

/// Copy textures from the first viewed level (default on; `XCL_HELD_LEVELS=0` whole resource).
pub fn held_levels() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_HELD_LEVELS", "VICTORY_HELD_LEVELS", "host-copy from first viewed level"))
}

/// Helper-lane ownership in emit (default on; `XCL_HELPER_OWN=0` off).
pub fn helper_own() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_HELPER_OWN", "VICTORY_HELPER_OWN", "helper-lane ownership on"))
}

/// Child-process marker for host-lock tests (`XCL_HOST_LOCK_CHILD` set).
pub fn host_lock_child() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_HOST_LOCK_CHILD", "VICTORY_HOST_LOCK_CHILD").is_some();
        if on {
            log_once("XCL_HOST_LOCK_CHILD", "host-lock child process");
        }
        on
    })
}

/// Translate as for a host whose waves are at most this many lanes (`XCL_HOST_MAX_WAVE`).
pub fn host_max_wave() -> Option<u32> {
    static N: OnceLock<Option<u32>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_HOST_MAX_WAVE", "VICTORY_HOST_MAX_WAVE")
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0);
        if let Some(n) = n {
            log_once("XCL_HOST_MAX_WAVE", &format!("{n} — translate for max host wave"));
        }
        n
    })
}

/// Translate as for an AMD RDNA host (`XCL_HOST_RDNA=1`).
pub fn host_rdna() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_HOST_RDNA", "VICTORY_HOST_RDNA", "1 — translate as RDNA host"))
}

/// Translate as for a GPU whose waves are at most this many lanes (`XCL_HOST_WAVE`).
pub fn host_wave() -> Option<u32> {
    static N: OnceLock<Option<u32>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_HOST_WAVE", "VICTORY_HOST_WAVE")
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0);
        if let Some(n) = n {
            log_once("XCL_HOST_WAVE", &format!("{n} — translate for host wave size"));
        }
        n
    })
}

// —— paths / tooling ——

/// Explicit config file (`XCL_CONFIG` / `VICTORY_CONFIG`).
pub fn config_path() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_CONFIG", "VICTORY_CONFIG").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_CONFIG", &format!("{}", path.display()));
        }
        p
    })
    .as_ref()
}

/// Console SDK `dxc.exe` for emit diffs (`XCL_CONSOLE_DXC`).
pub fn console_dxc() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_CONSOLE_DXC", "VICTORY_CONSOLE_DXC").map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_CONSOLE_DXC", &format!("{}", path.display()));
        }
        p
    })
    .as_ref()
}

/// Per-title data directory override (`XCL_DATA_DIR` / `VICTORY_DATA_DIR`).
pub fn data_dir() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_DATA_DIR", "VICTORY_DATA_DIR").map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_DATA_DIR", &format!("{}", path.display()));
        }
        p
    })
    .as_ref()
}

/// Filter emit-diff cases (`XCL_DIFF_ONLY`).
pub fn diff_only() -> Option<&'static str> {
    static S: OnceLock<Option<String>> = OnceLock::new();
    S.get_or_init(|| {
        let s = env_str("XCL_DIFF_ONLY", "VICTORY_DIFF_ONLY").filter(|v| !v.is_empty());
        if let Some(ref v) = s {
            log_once("XCL_DIFF_ONLY", v);
        }
        s
    })
    .as_deref()
}

/// Path to `dxil.dll` for the validator (`XCL_DXIL` / `VICTORY_DXIL`).
pub fn dxil_path() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_DXIL", "VICTORY_DXIL").map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_DXIL", &format!("{}", path.display()));
        }
        p
    })
    .as_ref()
}

/// Directory for emitted shader dumps (`XCL_EMIT_DUMP`).
pub fn emit_dump() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_EMIT_DUMP", "VICTORY_EMIT_DUMP").map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_EMIT_DUMP", &format!("{}", path.display()));
        }
        p
    })
    .as_ref()
}

/// Child-exit marker for loader re-exec (`XCL_EXIT_CHILD` set).
pub fn exit_child() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = env_os("XCL_EXIT_CHILD", "VICTORY_EXIT_CHILD").is_some();
        if on {
            log_once("XCL_EXIT_CHILD", "loader exit-child marker");
        }
        on
    })
}

/// Directory for first failing shader containers (`XCL_FAIL_DUMP`).
pub fn fail_dump() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_FAIL_DUMP", "VICTORY_FAIL_DUMP").map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_FAIL_DUMP", &format!("{}", path.display()));
        }
        p
    })
    .as_ref()
}

/// Fault-book output file (`XCL_FAULT_BOOK`).
pub fn fault_book() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_str("XCL_FAULT_BOOK", "VICTORY_FAULT_BOOK")
            .filter(|f| !f.is_empty())
            .map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_FAULT_BOOK", &format!("{}", path.display()));
        }
        p
    })
    .as_ref()
}

/// Seconds after start to sample the fault book (`XCL_FAULT_BOOK_AT`, default 150,210,270).
pub fn fault_book_at() -> &'static [u64] {
    static LIST: OnceLock<Vec<u64>> = OnceLock::new();
    LIST.get_or_init(|| {
        let raw = env_str("XCL_FAULT_BOOK_AT", "VICTORY_FAULT_BOOK_AT").unwrap_or_else(|| "150,210,270".into());
        let times = parse_u64_list(&raw);
        if env_str("XCL_FAULT_BOOK_AT", "VICTORY_FAULT_BOOK_AT").is_some() {
            log_once("XCL_FAULT_BOOK_AT", &raw);
        }
        if times.is_empty() {
            vec![150, 210, 270]
        } else {
            times
        }
    })
    .as_slice()
}

/// Touch boot/render-adjacent latches once (refresh / device init).
pub fn on_refresh_start() {
    let _ = (
        capture_frames().len(),
        capture_world(),
        capture_world_also().len(),
        clzero_ask_auto(),
        committed_query(),
        config_path(),
        console_dxc(),
        copy_ask(),
        data_dir(),
        declare_once(),
        desc_why(),
        diff_only(),
        dxil_path(),
        efficiency_mode(),
        emit_dump(),
        exit_child(),
        fail_dump(),
        fault_book(),
        fault_book_at().len(),
        fetch_check(),
        full_card_pass(),
        held_levels(),
        helper_own(),
        host_lock_child(),
        host_max_wave(),
        host_rdna(),
        host_wave(),
    );
    super::misc_env::on_refresh_start();
    super::bench_env::on_boot();
    let _ = (
        super::partial_fill::bind_cache(),
        super::partial_fill::bvh_builder(),
        super::partial_fill::dump_targets(),
        super::partial_fill::fast_cores(),
        super::partial_fill::heap_sync(),
        super::partial_fill::heap_sync_aside(),
        super::partial_fill::pipeline_wait_ms(),
        super::partial_fill::port_cbv_place(),
        super::partial_fill::rdna_float(),
        super::partial_fill::texture_budget_mb(),
        super::partial_fill::warp(),
    );
}
