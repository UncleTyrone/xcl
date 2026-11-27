//! Frame record / replay (`XCL_RECORD` / `XCL_REPLAY`). **Default off.**
//!
//! Deterministic submit/present traces for regression work. Writes `index.jsonl` plus optional
//! per-frame `frames/frame_NNNN.jsonl` with enough metadata to re-issue the same PC command lists
//! on a live queue when pointers are still valid (same process).
//!
//! **Not** `XCL_CAPTURE_FRAME`: that path records title command streams and shader dumps for one
//! frame; this path only tracks queue submits, fence signals, and present tokens.
//!
//! **Gaps vs CGBJ `victory-gpu/replay/*`:** no full heap/PSO/resource snap, no cross-process
//! list rehydration, no `PresentX` re-issue (token logged only), no CLI player. Binary
//! `XCLFRAME` v2 blobs (`XCL_RECORD_WIRE=1`) add heap metadata, bind ids, and title GPU VAs
//! (VICFRAME role, not layout).
//!
//! ### Environment (default off)
//!
//! | Variable | When set | Default |
//! |----------|----------|---------|
//! | `XCL_RECORD` | Output directory; enables record hooks | unset = off |
//! | `XCL_RECORD_AT` | Skip first N presents before recording | `0` |
//! | `XCL_RECORD_FRAMES` | Stop after N frames (unless `ALWAYS`) | `1` frame |
//! | `XCL_RECORD_ALWAYS` | Keep recording every frame | off |
//! | `XCL_RECORD_REFS` | Sidecar `refs/submit_*.ref.json` (pointer sets) | off |
//! | `XCL_RECORD_WIRE` | Also write `frames/frame_NNNN.bin` (`XCLFRAME` v2) | off |
//! | `XCL_REPLAY` | Load `index.jsonl` from directory; hook presents | unset = off |
//! | `XCL_REPLAY_WIRE` | Prefer `frames/frame_NNNN.bin` over per-frame JSONL | off |
//! | `XCL_REPLAY_AT` | Skip first N presents before replay | `0` |
//! | `XCL_REPLAY_EXECUTE` | Re-issue `ExecuteCommandLists` on live queue | on when `XCL_REPLAY` set |
//! | `XCL_REPLAY_EXECUTE=0` | Log-only replay | — |
//! | `XCL_REPLAY_FENCES` | Re-issue recorded queue `SignalX` fences | off |

use std::ffi::c_void;
use std::fs::{create_dir_all, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12Fence};

#[derive(Clone)]
struct Settings {
    record_dir: Option<PathBuf>,
    replay_dir: Option<PathBuf>,
    skip_presents: u64,
    max_frames: Option<u64>,
    always: bool,
    /// Sidecar ref files under `refs/` (addresses only, no command bytes).
    write_refs: bool,
    /// Binary `XCLFRAME` beside JSONL (`XCL_RECORD_WIRE=1`).
    write_wire: bool,
    /// Load per-frame `XCLFRAME` blobs when replaying (`XCL_REPLAY_WIRE=1`).
    replay_wire: bool,
    replay_execute: bool,
    replay_fences: bool,
    replay_skip_presents: u64,
}

fn parse_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn env_on(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

fn settings() -> &'static Settings {
    static S: OnceLock<Settings> = OnceLock::new();
    S.get_or_init(|| {
        let record_dir = std::env::var("XCL_RECORD")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let replay_dir = std::env::var("XCL_REPLAY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let always = env_on("XCL_RECORD_ALWAYS");
        // With `XCL_REPLAY` set, GPU re-issue is on unless `XCL_REPLAY_EXECUTE=0`.
        let replay_execute = if replay_dir.is_some() {
            !std::env::var("XCL_REPLAY_EXECUTE")
                .ok()
                .is_some_and(|v| matches!(v.trim(), "0" | "false" | "no" | "off"))
        } else {
            env_on("XCL_REPLAY_EXECUTE")
        };
        Settings {
            record_dir,
            replay_dir,
            skip_presents: parse_u64("XCL_RECORD_AT").unwrap_or(0),
            max_frames: parse_u64("XCL_RECORD_FRAMES"),
            always,
            write_refs: env_on("XCL_RECORD_REFS"),
            write_wire: env_on("XCL_RECORD_WIRE"),
            replay_wire: env_on("XCL_REPLAY_WIRE"),
            replay_execute,
            replay_fences: env_on("XCL_REPLAY_FENCES"),
            replay_skip_presents: parse_u64("XCL_REPLAY_AT").unwrap_or(0),
        }
    })
}

static LOGGED: OnceLock<()> = OnceLock::new();
static SESSION_META: AtomicBool = AtomicBool::new(false);
static PRESENTS: AtomicU64 = AtomicU64::new(0);
static SUBMITS: AtomicU64 = AtomicU64::new(0);
static FRAMES_RECORDED: AtomicU64 = AtomicU64::new(0);
static WRITER: Mutex<Option<std::fs::File>> = Mutex::new(None);

static FRAME_BUF: Mutex<Vec<RecordedEvent>> = Mutex::new(Vec::new());
static FRAME_BOUND_HEAP_PTRS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static KNOWN_HEAPS: Mutex<Vec<HeapEntry>> = Mutex::new(Vec::new());
static REPLAY_FRAMES: OnceLock<Mutex<ReplayCatalog>> = OnceLock::new();
static REPLAY_CURSOR: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct HeapEntry {
    ptr: usize,
    ty: u32,
    count: u32,
    flags: u32,
}

impl HeapEntry {
    fn from_wire(h: &super::frame_wire::HeapEntry) -> Self {
        Self {
            ptr: h.ptr,
            ty: h.ty,
            count: h.count,
            flags: h.flags,
        }
    }

    fn to_wire(&self) -> super::frame_wire::HeapEntry {
        super::frame_wire::HeapEntry {
            ptr: self.ptr,
            ty: self.ty,
            count: self.count,
            flags: self.flags,
        }
    }
}

fn merge_heap_ptr(dst: &mut Vec<usize>, xs: &[usize]) {
    for &p in xs {
        if p != 0 && !dst.contains(&p) {
            dst.push(p);
        }
    }
}

/// Called from `CreateDescriptorHeap` while `XCL_RECORD` is active.
pub fn note_descriptor_heap(ptr: usize, ty: u32, count: u32, flags: u32) {
    if settings().record_dir.is_none() || ptr == 0 {
        return;
    }
    let mut g = KNOWN_HEAPS.lock().unwrap();
    if let Some(e) = g.iter_mut().find(|e| e.ptr == ptr) {
        e.ty = ty;
        e.count = count;
        e.flags = flags;
    } else {
        g.push(HeapEntry { ptr, ty, count, flags });
    }
}

fn heap_meta_from_ptr(ptr: usize) -> Option<HeapEntry> {
    if ptr == 0 {
        return None;
    }
    let raw = ptr as *mut std::ffi::c_void;
    unsafe {
        let h = ID3D12DescriptorHeap::from_raw_borrowed(&raw)?;
        let d = h.GetDesc();
        Some(HeapEntry {
            ptr,
            ty: d.Type.0 as u32,
            count: d.NumDescriptors,
            flags: d.Flags.0 as u32,
        })
    }
}

fn heap_snapshot_for_frame(frame_bound: &[usize]) -> Vec<HeapEntry> {
    let mut out = KNOWN_HEAPS.lock().unwrap().clone();
    for &p in frame_bound {
        if out.iter().any(|e| e.ptr == p) {
            continue;
        }
        if let Some(m) = heap_meta_from_ptr(p) {
            out.push(m);
        }
    }
    out.sort_by_key(|e| e.ptr);
    out
}

fn submit_bind_meta(mapped: &[*mut std::ffi::c_void]) -> (Vec<usize>, Vec<usize>, Vec<u64>, Vec<usize>) {
    let mut psos = Vec::with_capacity(mapped.len());
    let mut rs_gfx = Vec::with_capacity(mapped.len());
    let mut gpu_vas = Vec::new();
    let mut bound_heaps = Vec::new();
    for &pc in mapped {
        let list = pc;
        let (pso, rs) = super::bind::frame_bind_ids(list);
        psos.push(pso);
        rs_gfx.push(rs);
        gpu_vas.extend(super::list_x::frame_touch_vas(list));
        bound_heaps.extend(super::list_x::frame_bound_heaps(list));
    }
    gpu_vas.sort_unstable();
    gpu_vas.dedup();
    bound_heaps.sort_unstable();
    bound_heaps.dedup();
    (psos, rs_gfx, gpu_vas, bound_heaps)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum RecordedEvent {
    #[serde(rename = "session")]
    Session { format: u32 },
    Submit {
        n: u64,
        queue: usize,
        lists: usize,
        addrs: Vec<usize>,
        pc_addrs: Vec<usize>,
        backing: Vec<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blob_refs: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        psos: Vec<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rs_gfx: Vec<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        gpu_vas: Vec<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bound_heaps: Vec<usize>,
    },
    #[serde(rename = "heapsnap")]
    HeapSnap { heaps: Vec<HeapEntry> },
    Signal {
        queue: usize,
        fence: usize,
        value: u64,
    },
    Present {
        frame: u64,
        token: u64,
        submits: u64,
    },
}

struct ReplayCatalog {
  frames: Vec<Vec<RecordedEvent>>,
}

fn write_session_meta(dir: &Path) {
    if SESSION_META.swap(true, Ordering::Relaxed) {
        return;
    }
    let body = serde_json::json!({
        "format": 2,
        "kind": "xcl_frame_replay",
        "distinct_from": "XCL_CAPTURE_FRAME",
        "events": ["submit", "heapsnap", "signal", "present"],
        "wire": super::frame_wire::VERSION,
    });
    let _ = std::fs::write(dir.join("session.json"), body.to_string());
    append_line(&serde_json::json!({"kind":"session","format":2}).to_string());
}

fn ensure_log() {
    LOGGED.get_or_init(|| {
        let s = settings();
        if let Some(dir) = &s.record_dir {
            if create_dir_all(dir).is_ok() {
                let _ = create_dir_all(dir.join("frames"));
                if s.write_refs {
                    let _ = create_dir_all(dir.join("refs"));
                }
                let path = dir.join("index.jsonl");
                if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
                    *WRITER.lock().unwrap() = Some(f);
                    write_session_meta(dir);
                    crate::xlog_info!("D3D12.X: frame record XCL_RECORD → {}", path.display());
                } else {
                    crate::xlog_warn!("D3D12.X: XCL_RECORD could not open {}", path.display());
                }
            }
        }
        if let Some(dir) = &s.replay_dir {
            match load_replay_catalog(dir) {
                Ok(cat) => {
                    let n = cat.frames.len();
                    let _ = REPLAY_FRAMES.get_or_init(|| Mutex::new(cat));
                    crate::xlog_info!(
                        "D3D12.X: XCL_REPLAY={} ({} frame(s), execute={}, fences={}, wire={})",
                        dir.display(),
                        n,
                        settings().replay_execute,
                        settings().replay_fences,
                        settings().replay_wire
                    );
                }
                Err(e) => {
                    crate::xlog_warn!("D3D12.X: XCL_REPLAY={}: load failed: {e}", dir.display());
                }
            }
        }
    });
}

fn recording_active() -> bool {
    let s = settings();
    let Some(_) = s.record_dir else {
        return false;
    };
    if !s.always {
        if let Some(max) = s.max_frames {
            if FRAMES_RECORDED.load(Ordering::Relaxed) >= max {
                return false;
            }
        } else if FRAMES_RECORDED.load(Ordering::Relaxed) >= 1 {
            return false;
        }
    }
    true
}

fn append_line(line: &str) {
    ensure_log();
    if let Ok(mut g) = WRITER.lock() {
        if let Some(f) = g.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn append_event(ev: &RecordedEvent) {
    if let Ok(line) = serde_json::to_string(ev) {
        append_line(&line);
    }
    if recording_active() {
        FRAME_BUF.lock().unwrap().push(ev.clone());
    }
}

fn events_to_wire(events: &[RecordedEvent]) -> Vec<super::frame_wire::WireEvent> {
    use super::frame_wire::WireEvent;
    events
        .iter()
        .filter_map(|ev| match ev {
            RecordedEvent::Session { format } => Some(WireEvent::Session { format: *format }),
            RecordedEvent::Submit {
                n,
                queue,
                lists,
                addrs,
                pc_addrs,
                backing,
                psos,
                rs_gfx,
                gpu_vas,
                bound_heaps,
                ..
            } => Some(WireEvent::Submit {
                n: *n,
                queue: *queue,
                lists: *lists,
                addrs: addrs.clone(),
                pc_addrs: pc_addrs.clone(),
                backing: backing.clone(),
                psos: psos.clone(),
                rs_gfx: rs_gfx.clone(),
                gpu_vas: gpu_vas.clone(),
                bound_heaps: bound_heaps.clone(),
            }),
            RecordedEvent::HeapSnap { heaps } => Some(WireEvent::HeapSnap {
                heaps: heaps.iter().map(HeapEntry::to_wire).collect(),
            }),
            RecordedEvent::Signal { queue, fence, value } => Some(WireEvent::Signal {
                queue: *queue,
                fence: *fence,
                value: *value,
            }),
            RecordedEvent::Present { frame, token, submits } => Some(WireEvent::Present {
                frame: *frame,
                token: *token,
                submits: *submits,
            }),
        })
        .collect()
}

fn recorded_from_wire(events: &[super::frame_wire::WireEvent]) -> Vec<RecordedEvent> {
    events
        .iter()
        .filter_map(|ev| match ev {
            super::frame_wire::WireEvent::Session { format } => {
                Some(RecordedEvent::Session { format: *format })
            }
            super::frame_wire::WireEvent::Submit {
                n,
                queue,
                lists,
                addrs,
                pc_addrs,
                backing,
                psos,
                rs_gfx,
                gpu_vas,
                bound_heaps,
            } => Some(RecordedEvent::Submit {
                n: *n,
                queue: *queue,
                lists: *lists,
                addrs: addrs.clone(),
                pc_addrs: pc_addrs.clone(),
                backing: backing.clone(),
                blob_refs: vec![],
                psos: psos.clone(),
                rs_gfx: rs_gfx.clone(),
                gpu_vas: gpu_vas.clone(),
                bound_heaps: bound_heaps.clone(),
            }),
            super::frame_wire::WireEvent::HeapSnap { heaps } => Some(RecordedEvent::HeapSnap {
                heaps: heaps.iter().map(HeapEntry::from_wire).collect(),
            }),
            super::frame_wire::WireEvent::Signal { queue, fence, value } => Some(RecordedEvent::Signal {
                queue: *queue,
                fence: *fence,
                value: *value,
            }),
            super::frame_wire::WireEvent::Present { frame, token, submits } => Some(RecordedEvent::Present {
                frame: *frame,
                token: *token,
                submits: *submits,
            }),
        })
        .collect()
}

fn flush_frame_file(frame: u64) {
    let s = settings();
    let Some(dir) = &s.record_dir else { return };
    let events = std::mem::take(&mut *FRAME_BUF.lock().unwrap());
    if events.is_empty() {
        return;
    }
    let path = dir.join("frames").join(format!("frame_{frame:04}.jsonl"));
    if let Ok(mut f) = OpenOptions::new().create(true).write(true).truncate(true).open(&path) {
        for ev in &events {
            if let Ok(line) = serde_json::to_string(ev) {
                let _ = writeln!(f, "{line}");
            }
        }
    }
    if s.write_wire {
        let wire_path = dir.join("frames").join(format!("frame_{frame:04}.bin"));
        let wire = events_to_wire(&events);
        if let Err(e) = super::frame_wire::write_frame(&wire_path, &wire) {
            crate::xlog_warn!("D3D12.X: XCL_RECORD_WIRE write {}: {e}", wire_path.display());
        }
    }
}

fn load_wire_frame(dir: &Path, frame: u64) -> Option<Vec<RecordedEvent>> {
    let path = dir.join("frames").join(format!("frame_{frame:04}.bin"));
    super::frame_wire::read_frame(&path)
        .ok()
        .map(|wire| recorded_from_wire(&wire))
}

fn load_jsonl_frame(dir: &Path, frame: u64) -> Option<Vec<RecordedEvent>> {
    let path = dir.join("frames").join(format!("frame_{frame:04}.jsonl"));
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line.ok()?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line).ok()?);
    }
    (!out.is_empty()).then_some(out)
}

fn load_replay_catalog(dir: &Path) -> Result<ReplayCatalog, String> {
    let index = dir.join("index.jsonl");
    let file = std::fs::File::open(&index).map_err(|e| format!("open {}: {e}", index.display()))?;
    let reader = BufReader::new(file);
    let mut frames: Vec<Vec<RecordedEvent>> = Vec::new();
    let mut current = Vec::new();
    let use_wire = settings().replay_wire;
    let mut frame_no = 0u64;
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let ev: RecordedEvent = serde_json::from_str(line).map_err(|e| format!("json: {e}"))?;
        match &ev {
            RecordedEvent::Session { .. } => {}
            RecordedEvent::Present { frame, .. } => {
                frame_no += 1;
                if use_wire {
                    if let Some(events) = load_wire_frame(dir, *frame).filter(|e| !e.is_empty()) {
                        frames.push(events);
                    } else if let Some(events) = load_jsonl_frame(dir, *frame).filter(|e| !e.is_empty()) {
                        frames.push(events);
                    } else {
                        current.push(ev);
                        frames.push(std::mem::take(&mut current));
                    }
                } else {
                    current.push(ev);
                    frames.push(std::mem::take(&mut current));
                }
                current.clear();
            }
            _ => {
                if !use_wire {
                    current.push(ev);
                }
            }
        }
    }
    if !use_wire && !current.is_empty() {
        frames.push(current);
    }
    if frames.is_empty() {
        return Err(if use_wire {
            "no replay frames (index.jsonl presents and no frames/frame_*.bin)".into()
        } else {
            "index.jsonl has no present boundaries".into()
        });
    }
    let _ = frame_no;
    Ok(ReplayCatalog { frames })
}

fn replay_catalog() -> Option<std::sync::MutexGuard<'static, ReplayCatalog>> {
    REPLAY_FRAMES.get().map(|m| m.lock().unwrap())
}

pub fn record_enabled() -> bool {
    settings().record_dir.is_some()
}

pub fn replay_enabled() -> bool {
    settings().replay_dir.is_some()
}

fn pointer_live(ptr: usize) -> bool {
    ptr != 0 && crate::gpu::shader::readable(ptr, 8)
}

fn write_ref_file(dir: &Path, name: &str, title: &[usize], pc: &[usize], backing: &[usize]) -> Option<String> {
    let path = dir.join("refs").join(name);
    let body = serde_json::json!({
        "title_lists": title,
        "pc_lists": pc,
        "backing": backing,
    });
    std::fs::write(&path, body.to_string()).ok()?;
    Some(path.strip_prefix(dir).ok()?.display().to_string())
}

/// Pick PC command lists to re-submit: recorded segments, re-expand title wrappers, or backing primaries.
fn resolve_replay_lists(addrs: &[usize], pc_addrs: &[usize], backing: &[usize]) -> Vec<*mut c_void> {
    if !pc_addrs.is_empty() && pc_addrs.iter().all(|&a| pointer_live(a)) {
        return pc_addrs.iter().map(|&a| a as *mut c_void).collect();
    }
    let mut expanded = Vec::new();
    for &title in addrs {
        if !pointer_live(title) {
            continue;
        }
        unsafe {
            super::thin::push_one(title as *mut c_void, &mut expanded);
        }
    }
    if !expanded.is_empty() && expanded.iter().all(|&p| pointer_live(p as usize)) {
        return expanded;
    }
    backing
        .iter()
        .filter(|&&b| pointer_live(b))
        .map(|&b| b as *mut c_void)
        .collect()
}

/// Title `ExecuteCommandLists` after `thin::map_lists` (title lists + PC segments).
pub fn on_submit(queue: *mut c_void, title_lists: &[*mut c_void], mapped: &[*mut c_void]) {
    ensure_log();
    if !recording_active() {
        return;
    }

    {
        let n = SUBMITS.fetch_add(1, Ordering::Relaxed) + 1;
        let addrs: Vec<usize> = title_lists.iter().map(|p| *p as usize).collect();
        let pc_addrs: Vec<usize> = mapped.iter().map(|p| *p as usize).collect();
        let backing: Vec<usize> = title_lists
            .iter()
            .map(|&l| super::title_list::backing(l).map(|b| b as usize).unwrap_or(0))
            .collect();
        let (psos, rs_gfx, gpu_vas, bound_heaps) = submit_bind_meta(mapped);
        merge_heap_ptr(&mut FRAME_BOUND_HEAP_PTRS.lock().unwrap(), &bound_heaps);
        let mut blob_refs = Vec::new();
        if settings().write_refs {
            if let Some(dir) = &settings().record_dir {
                let name = format!("submit_{n:06}.ref.json");
                if let Some(rel) = write_ref_file(dir, &name, &addrs, &pc_addrs, &backing) {
                    blob_refs.push(rel);
                }
            }
        }
        let ev = RecordedEvent::Submit {
            n,
            queue: queue as usize,
            lists: title_lists.len(),
            addrs,
            pc_addrs,
            backing,
            blob_refs,
            psos,
            rs_gfx,
            gpu_vas,
            bound_heaps,
        };
        append_event(&ev);
    }
}

/// After `PresentX` token is known.
pub fn on_present(token: u64) {
    ensure_log();
    let record = record_enabled();
    let replay = replay_enabled();
    if !record && !replay {
        return;
    }

    let p = PRESENTS.fetch_add(1, Ordering::Relaxed) + 1;
    if record && p > settings().skip_presents && recording_active() {
        {
            let frame = FRAMES_RECORDED.fetch_add(1, Ordering::Relaxed) + 1;
            {
                let bound = std::mem::take(&mut *FRAME_BOUND_HEAP_PTRS.lock().unwrap());
                let heaps = heap_snapshot_for_frame(&bound);
                if !heaps.is_empty() {
                    append_event(&RecordedEvent::HeapSnap { heaps });
                }
            }
            let ev = RecordedEvent::Present {
                frame,
                token,
                submits: SUBMITS.load(Ordering::Relaxed),
            };
            append_event(&ev);
            flush_frame_file(frame);
            if !settings().always {
                if settings().max_frames.is_none_or(|m| frame >= m) {
                    crate::xlog_info!(
                        "D3D12.X: frame record stopped after {frame} frame(s) (set XCL_RECORD_ALWAYS=1 to continue)"
                    );
                }
            }
        }
    }

    if replay {
        if p <= settings().replay_skip_presents {
            return;
        }
        let frame_idx = REPLAY_CURSOR.fetch_add(1, Ordering::Relaxed) as usize;
        try_replay_frame(frame_idx, token);
    }
}

/// Title queue `Signal` / `SignalX` after the driver call succeeds.
pub fn on_queue_signal(queue: *mut c_void, fence: *mut c_void, value: u64) {
    if !record_enabled() {
        return;
    }
    if !recording_active() {
        return;
    }
    let ev = RecordedEvent::Signal {
        queue: queue as usize,
        fence: fence as usize,
        value,
    };
    append_event(&ev);
}

fn try_replay_frame(frame_idx: usize, token: u64) {
    let Some(cat) = replay_catalog() else { return };
    let Some(events) = cat.frames.get(frame_idx) else {
        crate::xlog_info!(
            "D3D12.X: XCL_REPLAY: no recorded frame {frame_idx} (present token {token}) — stopping replay cursor"
        );
        return;
    };

    if !settings().replay_execute {
        crate::xlog_info!(
            "D3D12.X: XCL_REPLAY: frame {frame_idx} (token {token}): {} event(s) — execute off (XCL_REPLAY_EXECUTE=0)",
            events.len()
        );
        return;
    }

    let mut submits = 0u32;
    let mut signals = 0u32;
    let mut skipped = 0u32;

    for ev in events.iter() {
        match ev {
            RecordedEvent::Submit {
                queue,
                addrs,
                pc_addrs,
                backing,
                ..
            } => {
                if !pointer_live(*queue) {
                    skipped += 1;
                    continue;
                }
                let live = resolve_replay_lists(addrs, pc_addrs, backing);
                if live.is_empty() {
                    skipped += 1;
                    continue;
                }
                unsafe {
                    super::title_textures::own_work(|| {
                        super::real::execute_mapped_lists(*queue as *mut c_void, &live);
                    });
                }
                submits += 1;
            }
            RecordedEvent::Signal { queue, fence, value } => {
                if !settings().replay_fences {
                    continue;
                }
                if !pointer_live(*queue) || !pointer_live(*fence) {
                    skipped += 1;
                    continue;
                }
                unsafe {
                    let q = ID3D12CommandQueue::from_raw(*queue as *mut c_void);
                    let fe = ID3D12Fence::from_raw(*fence as *mut c_void);
                    if q.Signal(&fe, *value).is_ok() {
                        signals += 1;
                    } else {
                        skipped += 1;
                    }
                }
            }
            RecordedEvent::Present { frame, token: t, .. } => {
                crate::xlog_info!(
                    "D3D12.X: XCL_REPLAY: replayed frame idx {frame_idx} (recorded frame {frame}, token {t}); PresentX not re-issued"
                );
            }
            RecordedEvent::HeapSnap { heaps } => {
                crate::xlog_info!(
                    "D3D12.X: XCL_REPLAY: frame {frame_idx}: heap snap {} heap(s) (metadata only)",
                    heaps.len()
                );
            }
            RecordedEvent::Session { .. } => {}
        }
    }

    if submits > 0 || signals > 0 || skipped > 0 {
        crate::xlog_info!(
            "D3D12.X: XCL_REPLAY: frame {frame_idx}: {submits} submit(s), {signals} signal(s), {skipped} skipped (stale pointers or off)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_off_without_env() {
        if std::env::var_os("XCL_RECORD").is_none() {
            assert!(!record_enabled());
        }
    }

    #[test]
    fn event_json_roundtrip() {
        let ev = RecordedEvent::Submit {
            n: 1,
            queue: 0x1000,
            lists: 2,
            addrs: vec![1, 2],
            pc_addrs: vec![3, 4, 5],
            backing: vec![6, 0],
            blob_refs: vec![],
            psos: vec![0xA],
            rs_gfx: vec![0xB],
            gpu_vas: vec![0x1000],
            bound_heaps: vec![0xC],
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: RecordedEvent = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, RecordedEvent::Submit { n: 1, .. }));
    }

    #[test]
    fn wire_recorded_roundtrip_v2_fields() {
        let recorded = vec![
            RecordedEvent::Submit {
                n: 2,
                queue: 0x40,
                lists: 1,
                addrs: vec![1],
                pc_addrs: vec![2],
                backing: vec![2],
                blob_refs: vec![],
                psos: vec![3],
                rs_gfx: vec![4],
                gpu_vas: vec![0x5000],
                bound_heaps: vec![0x80],
            },
            RecordedEvent::HeapSnap {
                heaps: vec![HeapEntry {
                    ptr: 0x80,
                    ty: 0,
                    count: 16,
                    flags: 1,
                }],
            },
        ];
        let wire = events_to_wire(&recorded);
        let back = recorded_from_wire(&wire);
        assert_eq!(back, recorded);
    }
}