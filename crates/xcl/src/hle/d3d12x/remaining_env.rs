//! Remaining CGBJ `VICTORY_*` → `XCL_*` latches (GPL-2; study-only source).
//! Boot/render/diag name parity; light OnceLock accessors.

use std::sync::OnceLock;

fn env_os(xcl: &str, victory: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(xcl).or_else(|| std::env::var_os(victory))
}

fn default_on(xcl: &'static str, victory: &'static str) -> bool {
    env_os(xcl, victory).is_none_or(|v| v != "0")
}

fn opt_in_1(xcl: &'static str, victory: &'static str) -> bool {
    std::env::var(xcl).or_else(|_| std::env::var(victory)).is_ok_and(|v| v.trim() == "1")
}

fn env_u64(xcl: &'static str, victory: &'static str, default: u64) -> u64 {
    std::env::var(xcl).or_else(|_| std::env::var(victory)).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// `XCL_AB=1` opt-in latch.
pub fn ab() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_AB", "VICTORY_AB"))
}

/// `XCL_AUDIO_CAPTURE=1` opt-in latch.
pub fn audio_capture() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_AUDIO_CAPTURE", "VICTORY_AUDIO_CAPTURE"))
}

/// `XCL_AUDIO_CAPTURE_OF=1` opt-in latch.
pub fn audio_capture_of() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_AUDIO_CAPTURE_OF", "VICTORY_AUDIO_CAPTURE_OF"))
}

/// `XCL_BARRIER_TEST=1` opt-in latch.
pub fn barrier_test() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_BARRIER_TEST", "VICTORY_BARRIER_TEST"))
}

/// `XCL_BENCH_CS` numeric latch.
pub fn bench_cs() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_BENCH_CS", "VICTORY_BENCH_CS", 0))
}

/// `XCL_BENCH_FILE` numeric latch.
pub fn bench_file() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_BENCH_FILE", "VICTORY_BENCH_FILE", 0))
}

/// `XCL_BENCH_HOLES` numeric latch.
pub fn bench_holes() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_BENCH_HOLES", "VICTORY_BENCH_HOLES", 0))
}

/// `XCL_BENCH_N` numeric latch.
pub fn bench_n() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_BENCH_N", "VICTORY_BENCH_N", 0))
}

/// `XCL_BENCH_S` numeric latch.
pub fn bench_s() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_BENCH_S", "VICTORY_BENCH_S", 0))
}

/// `XCL_BENCH_THREADS` numeric latch.
pub fn bench_threads() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_BENCH_THREADS", "VICTORY_BENCH_THREADS", 0))
}

/// `XCL_BENCH_WARP` numeric latch.
pub fn bench_warp() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_BENCH_WARP", "VICTORY_BENCH_WARP", 0))
}

/// `XCL_BLOCK_WAY` (default on unless `=0`).
pub fn block_way() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_BLOCK_WAY", "VICTORY_BLOCK_WAY"))
}

/// `XCL_BOOK=1` opt-in latch.
pub fn book() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_BOOK", "VICTORY_BOOK"))
}

/// `XCL_BURY_LAG` (default on unless `=0`).
pub fn bury_lag() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_BURY_LAG", "VICTORY_BURY_LAG"))
}

/// `XCL_D3D12_DRED` (default on unless `=0`).
pub fn d3d12_dred() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_D3D12_DRED", "VICTORY_D3D12_DRED"))
}

/// `XCL_DUMP_MATCH=1` opt-in latch.
pub fn dump_match() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_DUMP_MATCH", "VICTORY_DUMP_MATCH"))
}

/// `XCL_FIRST_HALF` (default on unless `=0`).
pub fn first_half() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_FIRST_HALF", "VICTORY_FIRST_HALF"))
}

/// `XCL_INPLACE` (default on unless `=0`).
pub fn inplace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_INPLACE", "VICTORY_INPLACE"))
}

/// `XCL_INPLACE_MB` numeric latch.
pub fn inplace_mb() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_INPLACE_MB", "VICTORY_INPLACE_MB", 0))
}

/// `XCL_INPLACE_PROBE` (default on unless `=0`).
pub fn inplace_probe() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_INPLACE_PROBE", "VICTORY_INPLACE_PROBE"))
}

/// `XCL_INPLACE_PROBE_MB` numeric latch.
pub fn inplace_probe_mb() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_INPLACE_PROBE_MB", "VICTORY_INPLACE_PROBE_MB", 0))
}

/// `XCL_KEYS=1` opt-in latch.
pub fn keys() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_KEYS", "VICTORY_KEYS"))
}

/// `XCL_KNOWN_HOLES` (default on unless `=0`).
pub fn known_holes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_KNOWN_HOLES", "VICTORY_KNOWN_HOLES"))
}

/// `XCL_LIGHT_REFRESH` (default on unless `=0`).
pub fn light_refresh() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_LIGHT_REFRESH", "VICTORY_LIGHT_REFRESH"))
}

/// `XCL_MAP_THREADS` numeric latch.
pub fn map_threads() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_MAP_THREADS", "VICTORY_MAP_THREADS", 0))
}

/// `XCL_MEMORY_DUMP=1` opt-in latch.
pub fn memory_dump() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_MEMORY_DUMP", "VICTORY_MEMORY_DUMP"))
}

/// `XCL_MEMORY_MAP=1` opt-in latch.
pub fn memory_map() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_MEMORY_MAP", "VICTORY_MEMORY_MAP"))
}

/// `XCL_MERGE_RACE=1` opt-in latch.
pub fn merge_race() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_MERGE_RACE", "VICTORY_MERGE_RACE"))
}

/// `XCL_MESH_OUTPUT_STRICT` (default on unless `=0`).
pub fn mesh_output_strict() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_MESH_OUTPUT_STRICT", "VICTORY_MESH_OUTPUT_STRICT"))
}

/// `XCL_MESH_TEST=1` opt-in latch.
pub fn mesh_test() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_MESH_TEST", "VICTORY_MESH_TEST"))
}

/// `XCL_NONE_SUCH` (default on unless `=0`).
pub fn none_such() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_NONE_SUCH", "VICTORY_NONE_SUCH"))
}

/// `XCL_ONE_PAD` (default on unless `=0`).
pub fn one_pad() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_ONE_PAD", "VICTORY_ONE_PAD"))
}

/// `XCL_PAIRS` (default on unless `=0`).
pub fn pairs() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PAIRS", "VICTORY_PAIRS"))
}

/// `XCL_PANEL=1` opt-in latch.
pub fn panel() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_PANEL", "VICTORY_PANEL"))
}

/// `XCL_PC_DXC` (default on unless `=0`).
pub fn pc_dxc() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PC_DXC", "VICTORY_PC_DXC"))
}

/// `XCL_PICKED_MB` numeric latch.
pub fn picked_mb() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_PICKED_MB", "VICTORY_PICKED_MB", 0))
}

/// `XCL_PICKED_WRITES` (default on unless `=0`).
pub fn picked_writes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PICKED_WRITES", "VICTORY_PICKED_WRITES"))
}

/// `XCL_PLACED_FILLER` (default on unless `=0`).
pub fn placed_filler() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PLACED_FILLER", "VICTORY_PLACED_FILLER"))
}

/// `XCL_PLACED_ORDER` (default on unless `=0`).
pub fn placed_order() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_PLACED_ORDER", "VICTORY_PLACED_ORDER"))
}

/// `XCL_PROFILE=1` opt-in latch.
pub fn profile() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_PROFILE", "VICTORY_PROFILE"))
}

/// `XCL_QUICK_FIND` (default on unless `=0`).
pub fn quick_find() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_QUICK_FIND", "VICTORY_QUICK_FIND"))
}

/// `XCL_QUICK_MIRROR` (default on unless `=0`).
pub fn quick_mirror() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_QUICK_MIRROR", "VICTORY_QUICK_MIRROR"))
}

/// `XCL_RAW_VIEWS` (default on unless `=0`).
pub fn raw_views() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_RAW_VIEWS", "VICTORY_RAW_VIEWS"))
}

/// `XCL_RAYS=1` opt-in latch.
pub fn rays() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_RAYS", "VICTORY_RAYS"))
}

/// `XCL_REF_PS` (default on unless `=0`).
pub fn ref_ps() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_REF_PS", "VICTORY_REF_PS"))
}

/// `XCL_REF_VS` (default on unless `=0`).
pub fn ref_vs() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_REF_VS", "VICTORY_REF_VS"))
}

/// `XCL_RELAUNCHED_FROM` (default on unless `=0`).
pub fn relaunched_from() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_RELAUNCHED_FROM", "VICTORY_RELAUNCHED_FROM"))
}

/// `XCL_REPLAY_GBV=1` opt-in latch.
pub fn replay_gbv() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_REPLAY_GBV", "VICTORY_REPLAY_GBV"))
}

/// `XCL_REPLAY_PEEK=1` opt-in latch.
pub fn replay_peek() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_REPLAY_PEEK", "VICTORY_REPLAY_PEEK"))
}

/// `XCL_SAME_RECORDS` (default on unless `=0`).
pub fn same_records() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SAME_RECORDS", "VICTORY_SAME_RECORDS"))
}

/// `XCL_SAVE_CHILD=1` opt-in latch.
pub fn save_child() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_SAVE_CHILD", "VICTORY_SAVE_CHILD"))
}

/// `XCL_SAVE_OP` (default on unless `=0`).
pub fn save_op() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SAVE_OP", "VICTORY_SAVE_OP"))
}

/// `XCL_SAVE_STOP=1` opt-in latch.
pub fn save_stop() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_SAVE_STOP", "VICTORY_SAVE_STOP"))
}

/// `XCL_SCISSOR_TEST=1` opt-in latch.
pub fn scissor_test() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_SCISSOR_TEST", "VICTORY_SCISSOR_TEST"))
}

/// `XCL_SCRATCH_POOL` (default on unless `=0`).
pub fn scratch_pool() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SCRATCH_POOL", "VICTORY_SCRATCH_POOL"))
}

/// `XCL_SCRATCH_SPARE_MB` numeric latch.
pub fn scratch_spare_mb() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_SCRATCH_SPARE_MB", "VICTORY_SCRATCH_SPARE_MB", 0))
}

/// `XCL_SCREEN_WAIT_MS` numeric latch.
pub fn screen_wait_ms() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_SCREEN_WAIT_MS", "VICTORY_SCREEN_WAIT_MS", 0))
}

/// `XCL_SECTION_WINDOW` (default on unless `=0`).
pub fn section_window() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SECTION_WINDOW", "VICTORY_SECTION_WINDOW"))
}

/// `XCL_SLOT_CHECK` (default on unless `=0`).
pub fn slot_check() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SLOT_CHECK", "VICTORY_SLOT_CHECK"))
}

/// `XCL_SLOT_HOLDS` (default on unless `=0`).
pub fn slot_holds() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SLOT_HOLDS", "VICTORY_SLOT_HOLDS"))
}

/// `XCL_SNAPSHOT_POOL` (default on unless `=0`).
pub fn snapshot_pool() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SNAPSHOT_POOL", "VICTORY_SNAPSHOT_POOL"))
}

/// `XCL_SPLIT_WAITS` (default on unless `=0`).
pub fn split_waits() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SPLIT_WAITS", "VICTORY_SPLIT_WAITS"))
}

/// `XCL_STACKS` (default on unless `=0`).
pub fn stacks() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_STACKS", "VICTORY_STACKS"))
}

/// `XCL_STORE_BESIDE` (default on unless `=0`).
pub fn store_beside() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_STORE_BESIDE", "VICTORY_STORE_BESIDE"))
}

/// `XCL_STREAM_NOTICE` (default on unless `=0`).
pub fn stream_notice() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_STREAM_NOTICE", "VICTORY_STREAM_NOTICE"))
}

/// `XCL_STREAM_PRIORITY` (default on unless `=0`).
pub fn stream_priority() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_STREAM_PRIORITY", "VICTORY_STREAM_PRIORITY"))
}

/// `XCL_SWEEP` (default on unless `=0`).
pub fn sweep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_SWEEP", "VICTORY_SWEEP"))
}

/// `XCL_TEST_D3D_DEBUG=1` opt-in latch.
pub fn test_d3d_debug() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_TEST_D3D_DEBUG", "VICTORY_TEST_D3D_DEBUG"))
}

/// `XCL_TEST_ROUNDS=1` opt-in latch.
pub fn test_rounds() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_TEST_ROUNDS", "VICTORY_TEST_ROUNDS"))
}

/// `XCL_TEST_TITLES=1` opt-in latch.
pub fn test_titles() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_TEST_TITLES", "VICTORY_TEST_TITLES"))
}

/// `XCL_TEXTURE_GUARD` (default on unless `=0`).
pub fn texture_guard() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_GUARD", "VICTORY_TEXTURE_GUARD"))
}

/// `XCL_TEXTURE_HOT` (default on unless `=0`).
pub fn texture_hot() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_HOT", "VICTORY_TEXTURE_HOT"))
}

/// `XCL_TEXTURE_KEEP` (default on unless `=0`).
pub fn texture_keep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_KEEP", "VICTORY_TEXTURE_KEEP"))
}

/// `XCL_TEXTURE_ONCE` (default on unless `=0`).
pub fn texture_once() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_ONCE", "VICTORY_TEXTURE_ONCE"))
}

/// `XCL_TEXTURE_ROOM_PERCENT` numeric latch.
pub fn texture_room_percent() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_TEXTURE_ROOM_PERCENT", "VICTORY_TEXTURE_ROOM_PERCENT", 0))
}

/// `XCL_TEXTURE_SAME` (default on unless `=0`).
pub fn texture_same() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_SAME", "VICTORY_TEXTURE_SAME"))
}

/// `XCL_TEXTURE_STAGE_MB` numeric latch.
pub fn texture_stage_mb() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_TEXTURE_STAGE_MB", "VICTORY_TEXTURE_STAGE_MB", 0))
}

/// `XCL_TEXTURE_STAGE_MS` numeric latch.
pub fn texture_stage_ms() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_TEXTURE_STAGE_MS", "VICTORY_TEXTURE_STAGE_MS", 0))
}

/// `XCL_TEXTURE_SWEEP` (default on unless `=0`).
pub fn texture_sweep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TEXTURE_SWEEP", "VICTORY_TEXTURE_SWEEP"))
}

/// `XCL_THIN_THREADS` numeric latch.
pub fn thin_threads() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_THIN_THREADS", "VICTORY_THIN_THREADS", 0))
}

/// `XCL_THREAD_DIMENSIONS` (default on unless `=0`).
pub fn thread_dimensions() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_THREAD_DIMENSIONS", "VICTORY_THREAD_DIMENSIONS"))
}

/// `XCL_TILES_TEST_CARD` (default on unless `=0`).
pub fn tiles_test_card() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TILES_TEST_CARD", "VICTORY_TILES_TEST_CARD"))
}

/// `XCL_TIMELINE=1` opt-in latch.
pub fn timeline() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_TIMELINE", "VICTORY_TIMELINE"))
}

/// `XCL_TIMELINE_AT=1` opt-in latch.
pub fn timeline_at() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_TIMELINE_AT", "VICTORY_TIMELINE_AT"))
}

/// `XCL_TIMELINE_FRAMES` numeric latch.
pub fn timeline_frames() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_TIMELINE_FRAMES", "VICTORY_TIMELINE_FRAMES", 0))
}

/// `XCL_TIMER_LOG` (default on unless `=0`).
pub fn timer_log() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TIMER_LOG", "VICTORY_TIMER_LOG"))
}

/// `XCL_TITLE_PRIORITIES` (default on unless `=0`).
pub fn title_priorities() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TITLE_PRIORITIES", "VICTORY_TITLE_PRIORITIES"))
}

/// `XCL_TRAIL` (default on unless `=0`).
pub fn trail() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TRAIL", "VICTORY_TRAIL"))
}

/// `XCL_TRANSLATE_ONLY` (default on unless `=0`).
pub fn translate_only() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_TRANSLATE_ONLY", "VICTORY_TRANSLATE_ONLY"))
}

/// `XCL_TRAP_LOG=1` opt-in latch.
pub fn trap_log() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_TRAP_LOG", "VICTORY_TRAP_LOG"))
}

/// `XCL_UNDER_THREADS` numeric latch.
pub fn under_threads() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_UNDER_THREADS", "VICTORY_UNDER_THREADS", 0))
}

/// `XCL_UNDER_WIDE` (default on unless `=0`).
pub fn under_wide() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_UNDER_WIDE", "VICTORY_UNDER_WIDE"))
}

/// `XCL_UNDER_WORDS` (default on unless `=0`).
pub fn under_words() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_UNDER_WORDS", "VICTORY_UNDER_WORDS"))
}

/// `XCL_UNWATCHED_FRAMES` numeric latch.
pub fn unwatched_frames() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_UNWATCHED_FRAMES", "VICTORY_UNWATCHED_FRAMES", 0))
}

/// `XCL_UPCOPY_CHECK` (default on unless `=0`).
pub fn upcopy_check() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_UPCOPY_CHECK", "VICTORY_UPCOPY_CHECK"))
}

/// `XCL_UPLOAD_LOW` (default on unless `=0`).
pub fn upload_low() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_UPLOAD_LOW", "VICTORY_UPLOAD_LOW"))
}

/// `XCL_WAIT_ASK` (default on unless `=0`).
pub fn wait_ask() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WAIT_ASK", "VICTORY_WAIT_ASK"))
}

/// `XCL_WAIT_FORCE` (default on unless `=0`).
pub fn wait_force() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WAIT_FORCE", "VICTORY_WAIT_FORCE"))
}

/// `XCL_WAIT_RESIDENT` (default on unless `=0`).
pub fn wait_resident() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WAIT_RESIDENT", "VICTORY_WAIT_RESIDENT"))
}

/// `XCL_WALLS_TRACE=1` opt-in latch.
pub fn walls_trace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_WALLS_TRACE", "VICTORY_WALLS_TRACE"))
}

/// `XCL_WARM_SECONDS` numeric latch.
pub fn warm_seconds() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_WARM_SECONDS", "VICTORY_WARM_SECONDS", 0))
}

/// `XCL_WINDOW_IDLE_MS` numeric latch.
pub fn window_idle_ms() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_WINDOW_IDLE_MS", "VICTORY_WINDOW_IDLE_MS", 0))
}

/// `XCL_WINDOW_KB` numeric latch.
pub fn window_kb() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_WINDOW_KB", "VICTORY_WINDOW_KB", 0))
}

/// `XCL_WINDOW_MAX` numeric latch.
pub fn window_max() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_WINDOW_MAX", "VICTORY_WINDOW_MAX", 0))
}

/// `XCL_WINDOW_TAILS` (default on unless `=0`).
pub fn window_tails() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WINDOW_TAILS", "VICTORY_WINDOW_TAILS"))
}

/// `XCL_WINDOWS_BAR_MB` numeric latch.
pub fn windows_bar_mb() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| env_u64("XCL_WINDOWS_BAR_MB", "VICTORY_WINDOWS_BAR_MB", 0))
}

/// `XCL_WINDOWS_OWN` (default on unless `=0`).
pub fn windows_own() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WINDOWS_OWN", "VICTORY_WINDOWS_OWN"))
}

/// `XCL_WRITE_SKIP` (default on unless `=0`).
pub fn write_skip() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_WRITE_SKIP", "VICTORY_WRITE_SKIP"))
}

/// `XCL_XMA_DUMP=1` opt-in latch.
pub fn xma_dump() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| opt_in_1("XCL_XMA_DUMP", "VICTORY_XMA_DUMP"))
}

/// `XCL_XMA_REFERENCE` (default on unless `=0`).
pub fn xma_reference() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_XMA_REFERENCE", "VICTORY_XMA_REFERENCE"))
}

/// `XCL_XMA_STREAMS` (default on unless `=0`).
pub fn xma_streams() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| default_on("XCL_XMA_STREAMS", "VICTORY_XMA_STREAMS"))
}


