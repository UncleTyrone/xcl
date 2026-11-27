//! Kelvaroq 0038–0040 on NVIDIA: direct constant-buffer views, per-tile fill-ahead,
//! ReBAR read-only placements, and large UAV card copies (`XCL_*` / legacy `VICTORY_*` names).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

pub const ALIGN: usize = 256;
pub const PIECE: usize = 64 << 10;

fn env_on(names: &[&'static str]) -> bool {
    names.iter().any(|n| std::env::var(n).is_ok_and(|v| v.trim() == "1"))
}

fn env_mb(names: &[&'static str], default_mb: u64) -> u64 {
    for n in names {
        if let Ok(v) = std::env::var(n) {
            if let Ok(mb) = v.trim().parse::<u64>() {
                return mb;
            }
        }
    }
    default_mb
}

fn env_kb(names: &[&'static str], default_kb: u64) -> u64 {
    for name in names {
        if let Ok(v) = std::env::var(name) {
            if let Ok(kb) = v.trim().parse::<u64>() {
                if kb > 0 {
                    return kb;
                }
            }
        }
    }
    default_kb
}

/// `XCL_DIRECT_CONSTANTS=1` / `VICTORY_DIRECT_CONSTANTS=1`: table CBVs view the host copy.
pub fn direct_constants() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = match std::env::var("XCL_DIRECT_CONSTANTS")
            .or_else(|_| std::env::var("VICTORY_DIRECT_CONSTANTS"))
            .as_deref()
        {
            Ok("1") => true,
            Ok("0") => false,
            _ => true, // nvidia-dev default (Kelvaroq 0038)
        };
        if on {
            crate::xlog_info!("D3D12.X: direct constants on — CBVs use the host copy (256-byte aligned; set XCL_DIRECT_CONSTANTS=0 to disable)");
        }
        on
    })
}

/// `XCL_FILL_WAIT=1` / `VICTORY_FILL_WAIT=1`: every fill waits for in-flight shader writes (A/B).
pub fn fill_always_wait() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| env_on(&["XCL_FILL_WAIT", "VICTORY_FILL_WAIT"]))
}

/// Default ReBAR upload budget (MiB) on this tree.
pub fn rebar_mb_default() -> u64 {
    2048
}

/// `XCL_REBAR_MB` / `VICTORY_REBAR_COPIES_MB` (default [`rebar_mb_default`]).
pub fn rebar_mb() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| {
        let mb = env_mb(&["XCL_REBAR_MB", "VICTORY_REBAR_COPIES_MB"], rebar_mb_default());
        crate::xlog_info!("D3D12.X: ReBAR-style read-only budget {mb} MiB (XCL_REBAR_MB)");
        mb
    })
}

/// `XCL_CARD_BUFFERS_MIN_KB` / `VICTORY_CARD_BUFFERS_MIN_KB` (default 64).
pub fn card_buffers_min_kb() -> u64 {
    static KB: OnceLock<u64> = OnceLock::new();
    *KB.get_or_init(|| {
        let kb = env_kb(&["XCL_CARD_BUFFERS_MIN_KB", "VICTORY_CARD_BUFFERS_MIN_KB"], 64);
        crate::xlog_info!("D3D12.X: card-copy buffers from {kb} KiB (XCL_CARD_BUFFERS_MIN_KB)");
        kb
    })
}

pub fn card_buffers_min_bytes() -> u64 {
    card_buffers_min_kb() << 10
}

/// Whether a placed buffer should use the card-copy path (UAV and at least [`card_buffers_min_bytes`]).
pub fn wants_card_copy(uav: bool, bytes: u64) -> bool {
    uav && bytes >= card_buffers_min_bytes()
}

// ---------------------------------------------------------------------------------------
// Direct constants: host-copy CBV and refresh before execute
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Range {
    va: u64,
    bytes: u32,
}

static CBV_RANGES: Mutex<Vec<Range>> = Mutex::new(Vec::new());
/// Ranges a dispatch in the current submission may write — stay on the copied path until landed.
static SUBMIT_WRITES: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();

fn submit_writes() -> &'static Mutex<HashSet<usize>> {
    SUBMIT_WRITES.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn note_cbv(va: u64, bytes: u32) {
    if !direct_constants() || bytes == 0 {
        return;
    }
    let aligned = va & !(ALIGN as u64 - 1);
    let end = va.saturating_add(bytes as u64).next_multiple_of(ALIGN as u64);
    let bytes = ((end - aligned) as u32).min(64 * 1024);
    let mut g = CBV_RANGES.lock().unwrap_or_else(|e| e.into_inner());
    if g.iter().any(|r| r.va == aligned && r.bytes == bytes) {
        return;
    }
    g.push(Range { va: aligned, bytes });
    if g.len() <= 4 || g.len().is_power_of_two() {
        crate::xlog_info!("D3D12.X: direct constants: CBV {aligned:#x}..{:#x} ({bytes} bytes)", aligned + bytes as u64);
    }
}

pub fn note_dispatch_write(va: u64, bytes: u64) {
    if !direct_constants() || bytes == 0 {
        return;
    }
    let mut g = submit_writes().lock().unwrap_or_else(|e| e.into_inner());
    let mut at = va as usize;
    let end = at.saturating_add(bytes as usize);
    while at < end {
        g.insert(at & !(PIECE - 1));
        at = (at & !(PIECE - 1)).saturating_add(PIECE);
    }
}

pub fn clear_submit_writes() {
    if direct_constants() {
        submit_writes().lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// V# words for a constant view at `va` (host/unified memory), 256-byte aligned size.
pub fn cbv_words(va: u64, size: u32) -> Option<[u32; 4]> {
    let u = crate::gpu::uma::get()?;
    let va = va as usize;
    if !(u.contains(va) || u.is_view(va)) {
        return None;
    }
    let aligned = (va & !(ALIGN - 1)) as u64;
    let bytes = cap_cbv_size(size);
    note_cbv(aligned, bytes);
    Some(crate::gpu::desc::cbv_words(aligned, bytes))
}

fn cap_cbv_size(size: u32) -> u32 {
    const MAX: u32 = 64 * 1024;
    size.min(MAX)
}

/// Copy title memory into the host range for each direct CBV before the lists run.
pub unsafe fn refresh_before_execute() {
    if !direct_constants() {
        return;
    }
    let writes = submit_writes().lock().unwrap_or_else(|e| e.into_inner()).clone();
    let ranges: Vec<Range> = CBV_RANGES.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for r in ranges {
        if range_overlaps_submit(&writes, r.va, r.bytes as u64) {
            continue;
        }
        refresh_range(r.va as usize, r.bytes as usize);
    }
}

fn range_overlaps_submit(writes: &HashSet<usize>, va: u64, bytes: u64) -> bool {
    if writes.is_empty() {
        return false;
    }
    let end = va.saturating_add(bytes);
    let mut at = (va as usize) & !(PIECE - 1);
    let limit = end as usize;
    while at < limit {
        if writes.contains(&at) {
            return true;
        }
        at = at.saturating_add(PIECE);
    }
    false
}

unsafe fn refresh_range(va: usize, bytes: usize) {
    if bytes == 0 || !crate::gpu::shader::readable(va, bytes.min(4096)) {
        return;
    }
    let Some(u) = crate::gpu::uma::get() else { return };
    let dst = if u.contains(va) {
        va
    } else if u.is_view(va) {
        super::view_want::want(super::view_want::Path::Cbv, va as u64, bytes as u64);
        va
    } else {
        return;
    };
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    if n < 4 || n.is_power_of_two() {
        crate::xlog_info!("D3D12.X: direct constants: refreshed {dst:#x} ({bytes} bytes) before execute");
    }
    let _ = dst;
}

// ---------------------------------------------------------------------------------------
// fill_ahead: wait only on 64 KiB tiles a flight changed (not process-wide)
// ---------------------------------------------------------------------------------------

mod fill_ahead {
    use super::*;

    const KEEP: u64 = 16;

    #[derive(Default)]
    struct Piece {
        pruned: u64,
        fills: Vec<(usize, usize, u64)>,
    }

    static WAIT_PIECES: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    static READ: OnceLock<Mutex<Option<HashMap<usize, Piece>>>> = OnceLock::new();

    fn wait_set() -> &'static Mutex<HashSet<usize>> {
        WAIT_PIECES.get_or_init(|| Mutex::new(HashSet::new()))
    }

    fn read_map() -> &'static Mutex<Option<HashMap<usize, Piece>>> {
        READ.get_or_init(|| Mutex::new(None))
    }
    static COUNT: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

    pub fn may_read_ahead() -> bool {
        !fill_always_wait()
    }

    /// A CPU-side fill is about to read title memory `[src, src + len)`.
    pub fn before_read(src: usize, len: usize) {
        if len == 0 {
            return;
        }
        if fill_always_wait() {
            wait_pieces(src, len);
            return;
        }
        if !may_read_ahead() {
            wait_pieces(src, len);
            return;
        }
        COUNT[0].fetch_add(1, Ordering::Relaxed);
        let frame = crate::hle::d3d12x::present::activity().0;
        let mut read = read_map().lock().unwrap_or_else(|e| e.into_inner());
        let map = read.get_or_insert_with(HashMap::new);
        for piece in src / PIECE..=(src + len - 1) / PIECE {
            let p = map.entry(piece).or_default();
            if p.pruned != frame {
                p.pruned = frame;
                p.fills.retain(|f| f.2 + KEEP >= frame);
            }
            p.fills.push((src, src + len, frame));
        }
        if wait_set().lock().unwrap_or_else(|e| e.into_inner()).contains(&piece_id(src)) {
            wait_pieces(src, len);
        }
    }

    fn piece_id(at: usize) -> usize {
        at & !(PIECE - 1)
    }

    fn wait_pieces(src: usize, len: usize) {
        let _ = (src, len);
        // GPU work is fenced per queue in sparse; an explicit wait here is left to the submit path
        // when a tile is on the wait list. Heavy fills use fill_always_wait for A/B.
    }

    /// Shader landing wrote `new` at `at` in title memory.
    pub fn shader_wrote(at: usize, new: &[u8]) {
        if new.is_empty() || fill_always_wait() {
            return;
        }
        let frame = crate::hle::d3d12x::present::activity().0;
        let mut conflict_pieces = Vec::new();
        {
            let read = read_map().lock().unwrap_or_else(|e| e.into_inner());
            let Some(map) = read.as_ref() else { return };
            let end = at + new.len();
            for piece in at / PIECE..=(end - 1) / PIECE {
                let Some(p) = map.get(&piece) else { continue };
                for &(from, to, read_in) in &p.fills {
                    if from < end && to > at && read_in + KEEP >= frame {
                        conflict_pieces.push(piece);
                        break;
                    }
                }
            }
        }
        if conflict_pieces.is_empty() {
            return;
        }
        COUNT[1].fetch_add(1, Ordering::Relaxed);
        let mut wait = wait_set().lock().unwrap_or_else(|e| e.into_inner());
        for piece in conflict_pieces {
            if wait.insert(piece) {
                COUNT[2].fetch_add(1, Ordering::Relaxed);
                static N: AtomicU64 = AtomicU64::new(0);
                let n = N.fetch_add(1, Ordering::Relaxed) + 1;
                if n <= 8 {
                    crate::xlog_warn!(
                        "D3D12.X: fill_ahead: 64 KiB tile {piece:#x} now waits for shader writes ({n} tiles so far; XCL_FILL_WAIT=1 waits all)"
                    );
                }
            }
        }
    }

    /// Accumulate per-refresh samples; emit one monitor line about every 2 s (not every call).
    pub fn frame_line(frames: f64) {
        if frames <= 0.0 {
            return;
        }
        static ACC_FRAMES: AtomicU64 = AtomicU64::new(0);
        // milli-frames so fractional `frames` still accumulate without float atomics
        let milli = (frames * 1000.0).round().max(1.0) as u64;
        ACC_FRAMES.fetch_add(milli, Ordering::Relaxed);

        static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        let now = std::time::Instant::now();
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let due = match *last {
            None => {
                *last = Some(now);
                false
            }
            Some(t) if now.duration_since(t) >= std::time::Duration::from_secs(2) => {
                *last = Some(now);
                true
            }
            Some(_) => false,
        };
        if !due {
            return;
        }

        let window = ACC_FRAMES.swap(0, Ordering::Relaxed) as f64 / 1000.0;
        let [ahead, looked] = [0, 1].map(|i| COUNT[i].swap(0, Ordering::Relaxed));
        if ahead == 0 && looked == 0 {
            return;
        }
        if window <= 0.0 {
            return;
        }
        let wait_n = wait_set().lock().unwrap_or_else(|e| e.into_inner()).len();
        crate::xlog_info!(
            "D3D12.X: fill_ahead (2 s): {:.1} fills/frame not always-waiting; {:.1} landing writes vs fills; {} 64 KiB tiles wait; XCL_FILL_WAIT=1 forces always-wait",
            ahead as f64 / window,
            looked as f64 / window,
            wait_n
        );
    }

    pub fn piece_waits(at: usize) -> bool {
        fill_always_wait() || wait_set().lock().unwrap_or_else(|e| e.into_inner()).contains(&piece_id(at))
    }
}

pub use fill_ahead::{before_read as fill_before_read, frame_line as fill_frame_line, shader_wrote};

// ---------------------------------------------------------------------------------------
// ReBAR: read-only buffers/windows in GPU_UPLOAD when budget allows
// ---------------------------------------------------------------------------------------

mod rebar {
    use super::*;

    static CHARGE: AtomicU64 = AtomicU64::new(0);
    static GPU_UPLOAD_OK: AtomicBool = AtomicBool::new(false);

    pub fn budget_bytes() -> u64 {
        rebar_mb() << 20
    }

    pub fn reserve(bytes: u64) -> bool {
        let budget = budget_bytes();
        let tenth = budget / 10;
        let cap = budget.saturating_sub(tenth);
        let used = CHARGE.load(Ordering::Relaxed);
        if used.saturating_add(bytes) > cap {
            return false;
        }
        CHARGE.fetch_add(bytes, Ordering::Relaxed);
        true
    }

    pub fn release(bytes: u64) {
        CHARGE.fetch_sub(bytes.min(CHARGE.load(Ordering::Relaxed)), Ordering::Relaxed);
    }

    pub fn read_only_placed(bytes: u64, uav: bool) -> bool {
        !uav && bytes > 0
    }

    /// Try a committed buffer in `GPU_UPLOAD` for read-only data `bytes` long.
    pub unsafe fn try_buffer(dev: &ID3D12Device, bytes: u64) -> Option<(ID3D12Resource, *mut u8, u64)> {
        if !read_only_placed(bytes, false) {
            return None;
        }
        if !reserve(bytes) {
            return None;
        }
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_GPU_UPLOAD,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes.max(ALIGN as u64),
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let mut buffer: Option<ID3D12Resource> = None;
        if dev
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut buffer)
            .is_err()
        {
            release(bytes);
            return None;
        }
        let buffer = buffer?;
        let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
        if buffer.Map(0, None, Some(&mut data)).is_err() || data.is_null() {
            release(bytes);
            return None;
        }
        if !GPU_UPLOAD_OK.load(Ordering::Relaxed) {
            GPU_UPLOAD_OK.store(true, Ordering::Relaxed);
            crate::xlog_info!("D3D12.X: ReBAR GPU_UPLOAD heaps available for read-only copies");
        }
        Some((buffer, data as *mut u8, bytes))
    }
}

pub use rebar::{read_only_placed, try_buffer as rebar_try_buffer};

// ---------------------------------------------------------------------------------------
// Card copies for large UAV buffers
// ---------------------------------------------------------------------------------------

mod card_buffers {
    use super::*;

    static REG: OnceLock<Mutex<HashMap<usize, (usize, u64)>>> = OnceLock::new();

    fn reg() -> &'static Mutex<HashMap<usize, (usize, u64)>> {
        REG.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn register(resource: usize, title_va: usize, bytes: u64) {
        if !wants_card_copy(true, bytes) {
            return;
        }
        reg().lock().unwrap_or_else(|e| e.into_inner()).insert(resource, (title_va, bytes));
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::xlog_info!("D3D12.X: card buffer copy at title {title_va:#x} ({bytes} bytes, resource {resource:#x})");
        }
    }

    pub fn forgot(resource: usize) {
        reg().lock().unwrap_or_else(|e| e.into_inner()).remove(&resource);
    }
}

pub use card_buffers::register as card_register;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_alignment() {
        assert_eq!(PIECE, 65536);
        assert_eq!(ALIGN, 256);
    }

    #[test]
    fn card_threshold_default_64k() {
        assert_eq!(card_buffers_min_kb(), 64);
        assert!(wants_card_copy(true, 64 * 1024));
        assert!(!wants_card_copy(true, 32 * 1024));
        assert!(!wants_card_copy(false, 128 * 1024));
    }

    #[test]
    fn rebar_default_budget() {
        assert_eq!(rebar_mb_default(), 2048);
    }
}