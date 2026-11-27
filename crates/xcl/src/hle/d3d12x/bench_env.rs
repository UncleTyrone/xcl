//! Bench / test / diag env latches (`XCL_BENCH_*`, A/B, timeline, …).
//!
//! CGBJ `VICTORY_*` parity; GPL-2 reimplementation (study-only @ 80e8619).
#![allow(dead_code)] // scrape-parity latches; call sites adopt gradually


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

fn opt_set(xcl: &'static str, victory: &'static str, detail: &str) -> bool {
    let on = env_os(xcl, victory).is_some_and(|v| !v.is_empty() && v != "0");
    if on {
        log_once(xcl, detail);
    }
    on
}


/// `XCL_A` set: A/B side A switches.
pub fn a() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_A", "VICTORY_A", "A/B side A switches"))
}


/// `XCL_AB` set: A/B timed switch schedule.
pub fn ab() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_AB", "VICTORY_AB", "A/B timed switch schedule"))
}


/// `XCL_B` set: A/B side B switches.
pub fn b() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_B", "VICTORY_B", "A/B side B switches"))
}


/// `XCL_NONE_SUCH` set: A/B none-such marker.
pub fn none_such() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_NONE_SUCH", "VICTORY_NONE_SUCH", "A/B none-such marker"))
}


/// `XCL_AUDIO_CAPTURE` set: capture title audio.
pub fn audio_capture() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_AUDIO_CAPTURE", "VICTORY_AUDIO_CAPTURE", "capture title audio"))
}


/// `XCL_AUDIO_CAPTURE_OF` set: audio capture filter.
pub fn audio_capture_of() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_AUDIO_CAPTURE_OF", "VICTORY_AUDIO_CAPTURE_OF", "audio capture filter"))
}


/// `XCL_BARRIER_TEST=1`: run barrier timing tests.
pub fn barrier_test() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_BARRIER_TEST", "VICTORY_BARRIER_TEST", "1 — run barrier timing tests"))
}


/// `XCL_BENCH_CS` path: extra compute shader to time.
pub fn bench_cs() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_BENCH_CS", "VICTORY_BENCH_CS").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_BENCH_CS", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_BENCH_FILE` path: bench output file.
pub fn bench_file() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_BENCH_FILE", "VICTORY_BENCH_FILE").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_BENCH_FILE", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_BENCH_HOLES` (default on; `=0` off).
pub fn bench_holes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_BENCH_HOLES", "VICTORY_BENCH_HOLES", "include holes in bench"))
}


/// `XCL_BENCH_N` optional integer: bench iteration N.
pub fn bench_n() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_BENCH_N", "VICTORY_BENCH_N").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_BENCH_N", &format!("{n} — bench iteration N"));
        }
        n
    })
}


/// `XCL_BENCH_S` optional integer: bench seconds.
pub fn bench_s() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_BENCH_S", "VICTORY_BENCH_S").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_BENCH_S", &format!("{n} — bench seconds"));
        }
        n
    })
}


/// `XCL_BENCH_THREADS` optional integer: bench worker threads.
pub fn bench_threads() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_BENCH_THREADS", "VICTORY_BENCH_THREADS").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_BENCH_THREADS", &format!("{n} — bench worker threads"));
        }
        n
    })
}


/// `XCL_BENCH_WARP=1`: run bench on WARP.
pub fn bench_warp() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_BENCH_WARP", "VICTORY_BENCH_WARP", "1 — run bench on WARP"))
}


/// `XCL_BLOCK_WAY` optional integer: indirect block way.
pub fn block_way() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_BLOCK_WAY", "VICTORY_BLOCK_WAY").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_BLOCK_WAY", &format!("{n} — indirect block way"));
        }
        n
    })
}


/// `XCL_KEYS` set: dump input key diagnostics.
pub fn keys() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_KEYS", "VICTORY_KEYS", "dump input key diagnostics"))
}


/// `XCL_MESH_TEST=1`: mesh pipeline self-test.
pub fn mesh_test() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_MESH_TEST", "VICTORY_MESH_TEST", "1 — mesh pipeline self-test"))
}


/// `XCL_ONE_PAD` set: single gamepad test mode.
pub fn one_pad() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_ONE_PAD", "VICTORY_ONE_PAD", "single gamepad test mode"))
}


/// `XCL_PAIRS` set: translate pair diagnostics.
pub fn pairs() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_PAIRS", "VICTORY_PAIRS", "translate pair diagnostics"))
}


/// `XCL_RELAUNCHED_FROM` set: relaunch marker.
pub fn relaunched_from() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_RELAUNCHED_FROM", "VICTORY_RELAUNCHED_FROM", "relaunch marker"))
}


/// `XCL_REPLAY_GBV=1`: replay with GPU-based validation.
pub fn replay_gbv() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_REPLAY_GBV", "VICTORY_REPLAY_GBV", "1 — replay with GPU-based validation"))
}


/// `XCL_REPLAY_PEEK=1`: peek replay divergence.
pub fn replay_peek() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_REPLAY_PEEK", "VICTORY_REPLAY_PEEK", "1 — peek replay divergence"))
}


/// `XCL_SAVE_CHILD` path: save child process marker.
pub fn save_child() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_SAVE_CHILD", "VICTORY_SAVE_CHILD").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_SAVE_CHILD", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_SAVE_OP` path: save operation dump path.
pub fn save_op() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_SAVE_OP", "VICTORY_SAVE_OP").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_SAVE_OP", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_SAVE_STOP` set: stop after save op.
pub fn save_stop() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_SAVE_STOP", "VICTORY_SAVE_STOP", "stop after save op"))
}


/// `XCL_SCISSOR_TEST=1`: scissor self-test.
pub fn scissor_test() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_SCISSOR_TEST", "VICTORY_SCISSOR_TEST", "1 — scissor self-test"))
}


/// `XCL_SECTION_WINDOW` set: section window diagnostics.
pub fn section_window() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_SECTION_WINDOW", "VICTORY_SECTION_WINDOW", "section window diagnostics"))
}


/// `XCL_TEST_D3D_DEBUG=1`: force D3D debug in tests.
pub fn test_d3d_debug() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_TEST_D3D_DEBUG", "VICTORY_TEST_D3D_DEBUG", "1 — force D3D debug in tests"))
}


/// `XCL_TEST_ROUNDS` optional integer: host test rounds.
pub fn test_rounds() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_TEST_ROUNDS", "VICTORY_TEST_ROUNDS").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_TEST_ROUNDS", &format!("{n} — host test rounds"));
        }
        n
    })
}


/// `XCL_TEST_TITLES` path: test titles list.
pub fn test_titles() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        let p = env_os("XCL_TEST_TITLES", "VICTORY_TEST_TITLES").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(ref path) = p {
            log_once("XCL_TEST_TITLES", &format!("{}", path.display()));
        }
        p
    }).as_ref()
}


/// `XCL_TILES_TEST_CARD=1`: tiled resource test card.
pub fn tiles_test_card() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_TILES_TEST_CARD", "VICTORY_TILES_TEST_CARD", "1 — tiled resource test card"))
}


/// `XCL_TIMELINE` set: enable GPU timeline capture.
pub fn timeline() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_TIMELINE", "VICTORY_TIMELINE", "enable GPU timeline capture"))
}


/// `XCL_TIMELINE_AT` set: timeline sample times.
pub fn timeline_at() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_TIMELINE_AT", "VICTORY_TIMELINE_AT", "timeline sample times"))
}


/// `XCL_TIMELINE_FRAMES` set: timeline frame list.
pub fn timeline_frames() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_TIMELINE_FRAMES", "VICTORY_TIMELINE_FRAMES", "timeline frame list"))
}


/// `XCL_TITLE_PRIORITIES` set: override title thread priorities.
pub fn title_priorities() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_TITLE_PRIORITIES", "VICTORY_TITLE_PRIORITIES", "override title thread priorities"))
}


/// `XCL_WARM_SECONDS` optional integer: warm-up seconds before translate.
pub fn warm_seconds() -> Option<u64> {
    static N: OnceLock<Option<u64>> = OnceLock::new();
    *N.get_or_init(|| {
        let n = env_str("XCL_WARM_SECONDS", "VICTORY_WARM_SECONDS").and_then(|v| v.trim().parse().ok());
        if let Some(n) = n {
            log_once("XCL_WARM_SECONDS", &format!("{n} — warm-up seconds before translate"));
        }
        n
    })
}


/// `XCL_XMA_DUMP` set: dump XMA streams.
pub fn xma_dump() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_XMA_DUMP", "VICTORY_XMA_DUMP", "dump XMA streams"))
}


/// `XCL_XMA_REFERENCE` set: XMA reference compare.
pub fn xma_reference() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_XMA_REFERENCE", "VICTORY_XMA_REFERENCE", "XMA reference compare"))
}


/// `XCL_XMA_STREAMS=1`: force XMA stream path.
pub fn xma_streams() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_XMA_STREAMS", "VICTORY_XMA_STREAMS", "1 — force XMA stream path"))
}


/// `XCL_DUMP_MATCH` set: dump shader match diagnostics.
pub fn dump_match() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_DUMP_MATCH", "VICTORY_DUMP_MATCH", "dump shader match diagnostics"))
}


/// `XCL_TIMER_LOG` set: log frame timer samples.
pub fn timer_log() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_TIMER_LOG", "VICTORY_TIMER_LOG", "log frame timer samples"))
}


/// `XCL_TRAP_LOG` set: log trap / thunk faults.
pub fn trap_log() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_TRAP_LOG", "VICTORY_TRAP_LOG", "log trap / thunk faults"))
}


/// `XCL_PROFILE` set: enable profile sampling.
pub fn profile() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_PROFILE", "VICTORY_PROFILE", "enable profile sampling"))
}


/// `XCL_LOG` set: extra host logging.
pub fn log() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_LOG", "VICTORY_LOG", "extra host logging"))
}


/// `XCL_STACKS` set: capture stacks on profile.
pub fn stacks() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_set("XCL_STACKS", "VICTORY_STACKS", "capture stacks on profile"))
}

/// Touch latches once so log-once / scrape literals are live.
pub fn on_boot() {
    let _ = (
        a(),
        ab(),
        b(),
        none_such(),
        audio_capture(),
        audio_capture_of(),
        barrier_test(),
        bench_cs().is_some(),
        bench_file().is_some(),
        bench_holes(),
        bench_n().is_some(),
        bench_s().is_some(),
    );
    let _ = (
        bench_threads().is_some(),
        bench_warp(),
        block_way().is_some(),
        keys(),
        mesh_test(),
        one_pad(),
        pairs(),
        relaunched_from(),
        replay_gbv(),
        replay_peek(),
        save_child().is_some(),
        save_op().is_some(),
    );
    let _ = (
        save_stop(),
        scissor_test(),
        section_window(),
        test_d3d_debug(),
        test_rounds().is_some(),
        test_titles().is_some(),
        tiles_test_card(),
        timeline(),
        timeline_at(),
        timeline_frames(),
        title_priorities(),
        warm_seconds().is_some(),
    );
    let _ = (
        xma_dump(),
        xma_reference(),
        xma_streams(),
        dump_match(),
        timer_log(),
        trap_log(),
        profile(),
        log(),
        stacks(),
    );
}
