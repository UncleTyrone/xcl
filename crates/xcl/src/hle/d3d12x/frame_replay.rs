//! Frame record / replay (`XCL_RECORD` / `XCL_REPLAY`). **Default off.**
//!
//! Deterministic submit/present traces for regression work. Writes `index.jsonl` plus optional
//! per-frame `frames/frame_NNNN.jsonl` with enough metadata to re-issue the same PC command lists
//! on a live queue when pointers are still valid (same process).
//!
//! **Not** `XCL_CAPTURE_FRAME`: that path records title command streams and shader dumps for one
//! frame; this path tracks queue submits, fence signals, present tokens/planes, structured list
//! payloads, and title-memory content samples/chunks.
//!
//! ## VICFRAME role (GPL-2 `XCLFRAME` v5)
//!
//! | CGBJ role | XCL stand-in |
//! |-----------|--------------|
//! | `List.commands` | `ListPayload` ops blob (`XLST` + draw/dispatch/indirect/mesh notes) |
//! | `contents.bin` / `Content` | `ContentChunk` + `frames/frame_NNNN.contents.bin` |
//! | `DescriptorHeap` spine | `HeapSnap` + GPU/CPU start handles (no slot-run codec) |
//! | Present picture / PresentX | Present plane tail (resource + rect + GetDesc WH/format) + optional re-issue |
//! | Offline `victory replay` | `xcl replay` inspect + `--listing` / `--edit` / `--pixel` / `--diverge` |
//! | `--pixel` / `--diverge` / edit | Offline `frame_tools` on wire blobs (no live GPU texel probe) |
//!
//! **Cross-process (impossible on Windows host):** `ID3D12CommandList` / queue COM pointers and
//! title GPU VAs are meaningless after the recording process exits or the device is torn down.
//! There is no DXGI/D3D12 API to deserialize a VICFRAME/CGBJ command stream into live lists in a
//! fresh process; `xcl replay --run` only re-issues while the game is still running (`XCL_REPLAY`).
//!
//! **Honest limits:** full cross-process list rehydration from payload bytes alone is not
//! supported — Windows COM command-list handles and title GPU VAs are process-local. Same-process
//! replay re-issues live lists when pointers remain readable. CGBJ-scale resource/PSO/descriptor
//! snaps and GPU multi-run diverge / live pixel probe remain out of scope; offline content/plane
//! compare and ListPayload edit cover the product CLI surface.
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
//! | `XCL_RECORD_WIRE` | Also write `frames/frame_NNNN.bin` (`XCLFRAME` v5) | off |
//! | `XCL_RECORD_CONTENTS` | With wire: title-memory samples/chunks | on when `XCL_RECORD_WIRE` |
//! | `XCL_RECORD_CONTENTS=0` | Skip content samples (list meta/payload still written) | — |
//! | `XCL_RECORD_LISTS` | Structured list payloads + draw/dispatch ops | on when `XCL_RECORD_WIRE` |
//! | `XCL_REPLAY` | Load `index.jsonl` from directory; hook presents | unset = off |
//! | `XCL_REPLAY_WIRE` | Prefer `frames/frame_NNNN.bin` over per-frame JSONL | off |
//! | `XCL_REPLAY_AT` | Skip first N presents before replay | `0` |
//! | `XCL_REPLAY_EXECUTE` | Re-issue `ExecuteCommandLists` on live queue | on when `XCL_REPLAY` set |
//! | `XCL_REPLAY_EXECUTE=0` | Log-only replay | — |
//! | `XCL_REPLAY_FENCES` | Re-issue recorded queue `SignalX` fences | off |
//! | `XCL_REPLAY_CONTENTS` | Re-hash live title memory vs wire samples/chunks | on when `XCL_REPLAY_WIRE` |
//! | `XCL_REPLAY_PRESENT` | Re-issue `PresentX` with recorded planes (same process) | off |

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::{create_dir_all, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::{
    ID3D12CommandList, ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12Fence, ID3D12Resource,
};

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
    /// Title-memory content samples/chunks (`XCL_RECORD_CONTENTS`).
    write_contents: bool,
    /// Structured list payloads + live draw/dispatch ops (`XCL_RECORD_LISTS`).
    write_lists: bool,
    /// Load per-frame `XCLFRAME` blobs when replaying (`XCL_REPLAY_WIRE=1`).
    replay_wire: bool,
    /// Re-hash live title memory against wire content events.
    replay_contents: bool,
    replay_execute: bool,
    replay_fences: bool,
    /// Re-issue PresentX from recorded planes (`XCL_REPLAY_PRESENT=1`).
    replay_present: bool,
    replay_skip_presents: u64,
}

fn parse_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn env_on(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

fn env_off_explicit(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|v| matches!(v.trim(), "0" | "false" | "no" | "off"))
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
        let write_wire = env_on("XCL_RECORD_WIRE");
        let replay_wire = env_on("XCL_REPLAY_WIRE");
        let replay_execute = if replay_dir.is_some() {
            !env_off_explicit("XCL_REPLAY_EXECUTE")
        } else {
            env_on("XCL_REPLAY_EXECUTE")
        };
        let write_contents = if write_wire {
            !env_off_explicit("XCL_RECORD_CONTENTS")
        } else {
            env_on("XCL_RECORD_CONTENTS")
        };
        let write_lists = if write_wire {
            !env_off_explicit("XCL_RECORD_LISTS")
        } else {
            env_on("XCL_RECORD_LISTS")
        };
        let replay_contents = if replay_wire {
            !env_off_explicit("XCL_REPLAY_CONTENTS")
        } else {
            env_on("XCL_REPLAY_CONTENTS")
        };
        Settings {
            record_dir,
            replay_dir,
            skip_presents: parse_u64("XCL_RECORD_AT").unwrap_or(0),
            max_frames: parse_u64("XCL_RECORD_FRAMES"),
            always,
            write_refs: env_on("XCL_RECORD_REFS"),
            write_wire,
            write_contents,
            write_lists,
            replay_wire,
            replay_contents,
            replay_execute,
            replay_fences: env_on("XCL_REPLAY_FENCES"),
            replay_present: env_on("XCL_REPLAY_PRESENT"),
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
static FRAME_CONTENTS_BIN: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static KNOWN_HEAPS: Mutex<Vec<HeapEntry>> = Mutex::new(Vec::new());
static LIST_OPS: OnceLock<Mutex<HashMap<usize, Vec<u8>>>> = OnceLock::new();
static PENDING_PRESENT: Mutex<Option<PendingPresent>> = Mutex::new(None);
static REPLAY_FRAMES: OnceLock<Mutex<ReplayCatalog>> = OnceLock::new();
static REPLAY_CURSOR: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
struct PendingPresent {
    queue: usize,
    planes: Vec<PresentPlaneRec>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PresentPlaneRec {
    resource: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rect: Option<[i32; 4]>,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    format: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct HeapEntry {
    ptr: usize,
    ty: u32,
    count: u32,
    flags: u32,
    #[serde(default)]
    gpu_start: u64,
    #[serde(default)]
    cpu_start: u64,
}

impl HeapEntry {
    fn from_wire(h: &super::frame_wire::HeapEntry) -> Self {
        Self {
            ptr: h.ptr,
            ty: h.ty,
            count: h.count,
            flags: h.flags,
            gpu_start: h.gpu_start,
            cpu_start: h.cpu_start,
        }
    }

    fn to_wire(&self) -> super::frame_wire::HeapEntry {
        super::frame_wire::HeapEntry {
            ptr: self.ptr,
            ty: self.ty,
            count: self.count,
            flags: self.flags,
            gpu_start: self.gpu_start,
            cpu_start: self.cpu_start,
        }
    }
}

fn plane_desc(resource: usize) -> (u32, u32, u32) {
    if resource == 0 {
        return (0, 0, 0);
    }
    let raw = resource as *mut std::ffi::c_void;
    unsafe {
        let Some(res) = ID3D12Resource::from_raw_borrowed(&raw) else {
            return (0, 0, 0);
        };
        let d = res.GetDesc();
        (d.Width as u32, d.Height, d.Format.0 as u32)
    }
}

fn heap_starts(heap: &ID3D12DescriptorHeap) -> (u64, u64) {
    unsafe {
        (
            heap.GetGPUDescriptorHandleForHeapStart().ptr as u64,
            heap.GetCPUDescriptorHandleForHeapStart().ptr as u64,
        )
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
    let (gpu_start, cpu_start) = {
        let raw = ptr as *mut std::ffi::c_void;
        unsafe {
            ID3D12DescriptorHeap::from_raw_borrowed(&raw)
                .map(|h| heap_starts(h))
                .unwrap_or((0, 0))
        }
    };
    let mut g = KNOWN_HEAPS.lock().unwrap();
    if let Some(e) = g.iter_mut().find(|e| e.ptr == ptr) {
        e.ty = ty;
        e.count = count;
        e.flags = flags;
        e.gpu_start = gpu_start;
        e.cpu_start = cpu_start;
    } else {
        g.push(HeapEntry {
            ptr,
            ty,
            count,
            flags,
            gpu_start,
            cpu_start,
        });
    }
}

/// Whether draw/dispatch hooks should append into the list-op ring.
pub(super) fn list_ops_recording() -> bool {
    settings().record_dir.is_some() && settings().write_lists && recording_active()
}

fn list_ops() -> &'static Mutex<HashMap<usize, Vec<u8>>> {
    LIST_OPS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn append_list_op(list: usize, bytes: &[u8]) {
    if list == 0 || bytes.is_empty() {
        return;
    }
    let mut g = list_ops().lock().unwrap();
    let slot = g.entry(list).or_default();
    // Cap per-list op ring so a long frame cannot balloon process memory.
    const CAP: usize = 1 << 20;
    if slot.len() + bytes.len() > CAP {
        return;
    }
    slot.extend_from_slice(bytes);
}

pub(super) fn take_list_ops(list: usize) -> Vec<u8> {
    list_ops().lock().unwrap().remove(&list).unwrap_or_default()
}

/// Called from `PresentX` before `mark_flipped` so the next present event carries planes.
pub fn note_present_context(queue: *mut c_void, planes: &[(usize, Option<[i32; 4]>)]) {
    if settings().record_dir.is_none() && settings().replay_dir.is_none() {
        return;
    }
    let rec = PendingPresent {
        queue: queue as usize,
        planes: planes
            .iter()
            .map(|(r, rect)| {
                let (width, height, format) = plane_desc(*r);
                PresentPlaneRec {
                    resource: *r,
                    rect: *rect,
                    width,
                    height,
                    format,
                }
            })
            .collect(),
    };
    *PENDING_PRESENT.lock().unwrap() = Some(rec);
}

fn heap_meta_from_ptr(ptr: usize) -> Option<HeapEntry> {
    if ptr == 0 {
        return None;
    }
    let raw = ptr as *mut std::ffi::c_void;
    unsafe {
        let h = ID3D12DescriptorHeap::from_raw_borrowed(&raw)?;
        let d = h.GetDesc();
        let (gpu_start, cpu_start) = heap_starts(h);
        Some(HeapEntry {
            ptr,
            ty: d.Type.0 as u32,
            count: d.NumDescriptors,
            flags: d.Flags.0 as u32,
            gpu_start,
            cpu_start,
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

fn list_kind(pc: *mut c_void) -> u32 {
    unsafe {
        ID3D12CommandList::from_raw_borrowed(&pc)
            .map(|cl| cl.GetType().0 as u32)
            .unwrap_or(u32::MAX)
    }
}

fn list_meta_events(mapped: &[*mut c_void]) -> Vec<RecordedEvent> {
    let mut out = Vec::with_capacity(mapped.len());
    for &pc in mapped {
        let (pso, rs) = super::bind::frame_bind_ids(pc);
        let mut heaps = super::list_x::frame_bound_heaps(pc);
        heaps.sort_unstable();
        heaps.dedup();
        let mut vas = super::list_x::frame_touch_vas(pc);
        vas.sort_unstable();
        vas.dedup();
        out.push(RecordedEvent::ListMeta {
            list: pc as usize,
            list_kind: list_kind(pc),
            pso,
            rs_gfx: rs,
            heaps,
            vas,
        });
    }
    out
}

fn list_payload_events(mapped: &[*mut c_void]) -> Vec<RecordedEvent> {
    let mut out = Vec::with_capacity(mapped.len());
    for &pc in mapped {
        let mut ops = super::list_x::frame_list_payload(pc);
        let live = super::list_x::take_frame_ops(pc);
        if !live.is_empty() {
            ops.extend_from_slice(&live);
        }
        out.push(RecordedEvent::ListPayload {
            list: pc as usize,
            list_kind: list_kind(pc),
            ops,
        });
    }
    out
}

/// Sample title memory at `va` for wire digests (CGBJ `Content` role).
fn content_events_at(va: u64, contents_bin: &mut Vec<u8>) -> Vec<RecordedEvent> {
    let p = va as usize;
    if p == 0 {
        return Vec::new();
    }
    let want = super::frame_wire::CONTENT_CHUNK_CAP;
    let hash_len = if crate::gpu::shader::readable(p, want) {
        want
    } else if crate::gpu::shader::readable(p, super::frame_wire::CONTENT_SAMPLE_CAP) {
        super::frame_wire::CONTENT_SAMPLE_CAP
    } else if crate::gpu::shader::readable(p, 64) {
        64
    } else {
        return Vec::new();
    };
    let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, hash_len) };
    let hash = super::frame_wire::fnv1a32(bytes);
    let sample = bytes[..hash_len.min(super::frame_wire::CONTENT_SAMPLE_CAP)].to_vec();
    let mut out = vec![RecordedEvent::ContentSample {
        va,
        len: hash_len as u32,
        hash,
        sample,
    }];
    if contents_bin.len() + hash_len <= super::frame_wire::CONTENTS_BIN_SOFT_CAP {
        let file_off = contents_bin.len() as u64;
        contents_bin.extend_from_slice(bytes);
        out.push(RecordedEvent::ContentChunk {
            va,
            len: hash_len as u32,
            hash,
            file_off,
        });
    }
    out
}

fn content_sample_events(vas: &[u64]) -> Vec<RecordedEvent> {
    const MAX_VAS: usize = 128;
    let mut bin = FRAME_CONTENTS_BIN.lock().unwrap();
    let mut out = Vec::new();
    for &va in vas.iter().take(MAX_VAS) {
        out.extend(content_events_at(va, &mut bin));
    }
    out
}

fn verify_content_sample(va: u64, len: u32, want_hash: u32) -> Option<bool> {
    let p = va as usize;
    let n = len as usize;
    if p == 0 || n == 0 || n > 16 << 20 {
        return None;
    }
    if !crate::gpu::shader::readable(p, n) {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, n) };
    Some(super::frame_wire::fnv1a32(bytes) == want_hash)
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
    #[serde(rename = "listmeta")]
    ListMeta {
        list: usize,
        /// `D3D12_COMMAND_LIST_TYPE` (named `list_kind` so it does not clash with the serde tag).
        list_kind: u32,
        pso: usize,
        rs_gfx: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        heaps: Vec<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        vas: Vec<u64>,
    },
    #[serde(rename = "listpayload")]
    ListPayload {
        list: usize,
        list_kind: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ops: Vec<u8>,
    },
    #[serde(rename = "contentsample")]
    ContentSample {
        va: u64,
        len: u32,
        hash: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sample: Vec<u8>,
    },
    #[serde(rename = "contentchunk")]
    ContentChunk {
        va: u64,
        len: u32,
        hash: u32,
        file_off: u64,
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
        #[serde(default, skip_serializing_if = "is_zero_usize")]
        queue: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        planes: Vec<PresentPlaneRec>,
    },
}

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

struct ReplayCatalog {
    frames: Vec<Vec<RecordedEvent>>,
}

fn write_session_meta(dir: &Path) {
    if SESSION_META.swap(true, Ordering::Relaxed) {
        return;
    }
    let body = serde_json::json!({
        "format": 4,
        "kind": "xcl_frame_replay",
        "distinct_from": "XCL_CAPTURE_FRAME",
        "events": [
            "submit", "listmeta", "listpayload", "contentsample", "contentchunk",
            "heapsnap", "signal", "present"
        ],
        "wire": super::frame_wire::VERSION,
        "contents": settings().write_contents,
        "lists": settings().write_lists,
        "cross_process": "unsupported — COM list handles and title GPU VAs are process-local; ListPayload is diagnostic / same-process only",
    });
    let _ = std::fs::write(dir.join("session.json"), body.to_string());
    append_line(&serde_json::json!({"kind":"session","format":4}).to_string());
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
                        "D3D12.X: XCL_REPLAY={} ({} frame(s), execute={}, fences={}, present={}, wire={})",
                        dir.display(),
                        n,
                        settings().replay_execute,
                        settings().replay_fences,
                        settings().replay_present,
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
            RecordedEvent::ListMeta {
                list,
                list_kind,
                pso,
                rs_gfx,
                heaps,
                vas,
            } => Some(WireEvent::ListMeta {
                list: *list,
                kind: *list_kind,
                pso: *pso,
                rs_gfx: *rs_gfx,
                heaps: heaps.clone(),
                vas: vas.clone(),
            }),
            RecordedEvent::ListPayload { list, list_kind, ops } => Some(WireEvent::ListPayload {
                list: *list,
                kind: *list_kind,
                ops: ops.clone(),
            }),
            RecordedEvent::ContentSample { va, len, hash, sample } => Some(WireEvent::ContentSample {
                va: *va,
                len: *len,
                hash: *hash,
                sample: sample.clone(),
            }),
            RecordedEvent::ContentChunk {
                va,
                len,
                hash,
                file_off,
            } => Some(WireEvent::ContentChunk {
                va: *va,
                len: *len,
                hash: *hash,
                file_off: *file_off,
            }),
            RecordedEvent::HeapSnap { heaps } => Some(WireEvent::HeapSnap {
                heaps: heaps.iter().map(HeapEntry::to_wire).collect(),
            }),
            RecordedEvent::Signal { queue, fence, value } => Some(WireEvent::Signal {
                queue: *queue,
                fence: *fence,
                value: *value,
            }),
            RecordedEvent::Present {
                frame,
                token,
                submits,
                queue,
                planes,
            } => Some(WireEvent::Present {
                frame: *frame,
                token: *token,
                submits: *submits,
                queue: *queue,
                planes: planes
                    .iter()
                    .map(|p| super::frame_wire::PresentPlane {
                        resource: p.resource,
                        rect: p.rect,
                        width: p.width,
                        height: p.height,
                        format: p.format,
                    })
                    .collect(),
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
            super::frame_wire::WireEvent::ListMeta {
                list,
                kind,
                pso,
                rs_gfx,
                heaps,
                vas,
            } => Some(RecordedEvent::ListMeta {
                list: *list,
                list_kind: *kind,
                pso: *pso,
                rs_gfx: *rs_gfx,
                heaps: heaps.clone(),
                vas: vas.clone(),
            }),
            super::frame_wire::WireEvent::ListPayload { list, kind, ops } => {
                Some(RecordedEvent::ListPayload {
                    list: *list,
                    list_kind: *kind,
                    ops: ops.clone(),
                })
            }
            super::frame_wire::WireEvent::ContentSample { va, len, hash, sample } => {
                Some(RecordedEvent::ContentSample {
                    va: *va,
                    len: *len,
                    hash: *hash,
                    sample: sample.clone(),
                })
            }
            super::frame_wire::WireEvent::ContentChunk {
                va,
                len,
                hash,
                file_off,
            } => Some(RecordedEvent::ContentChunk {
                va: *va,
                len: *len,
                hash: *hash,
                file_off: *file_off,
            }),
            super::frame_wire::WireEvent::HeapSnap { heaps } => Some(RecordedEvent::HeapSnap {
                heaps: heaps.iter().map(HeapEntry::from_wire).collect(),
            }),
            super::frame_wire::WireEvent::Signal { queue, fence, value } => Some(RecordedEvent::Signal {
                queue: *queue,
                fence: *fence,
                value: *value,
            }),
            super::frame_wire::WireEvent::Present {
                frame,
                token,
                submits,
                queue,
                planes,
            } => Some(RecordedEvent::Present {
                frame: *frame,
                token: *token,
                submits: *submits,
                queue: *queue,
                planes: planes
                    .iter()
                    .map(|p| PresentPlaneRec {
                        resource: p.resource,
                        rect: p.rect,
                        width: p.width,
                        height: p.height,
                        format: p.format,
                    })
                    .collect(),
            }),
        })
        .collect()
}

static FRAME_NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Best-effort note for `frames/frame_NNNN.notes.txt` (CGBJ `notes.txt` role).
pub(crate) fn note_frame_problem(msg: impl Into<String>) {
    if let Ok(mut n) = FRAME_NOTES.lock() {
        n.push(msg.into());
    }
}

/// JSON sidecar for `XCL_CAPTURE` (`frame-<n>.json`).
pub fn capture_companion_json(boundary: u64) -> Option<String> {
    let s = settings();
    let body = serde_json::json!({
        "boundary": boundary,
        "xcl_record": s.record_dir.as_ref().map(|p| p.display().to_string()),
        "recording_active": record_enabled() && recording_active(),
        "wire": s.write_wire,
        "contents": s.write_contents,
        "lists": s.write_lists,
        "wire_version": super::frame_wire::VERSION,
        "cross_process": "unsupported",
    });
    Some(body.to_string())
}

fn flush_frame_file(frame: u64) {
    let s = settings();
    let Some(dir) = &s.record_dir else { return };
    let events = std::mem::take(&mut *FRAME_BUF.lock().unwrap());
    let contents = std::mem::take(&mut *FRAME_CONTENTS_BIN.lock().unwrap());
    let notes = std::mem::take(&mut *FRAME_NOTES.lock().unwrap());
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
    if s.write_contents && !contents.is_empty() {
        let bin_path = dir.join("frames").join(format!("frame_{frame:04}.contents.bin"));
        if let Err(e) = std::fs::write(&bin_path, &contents) {
            crate::xlog_warn!("D3D12.X: contents.bin write {}: {e}", bin_path.display());
        }
    }
    let notes_path = dir.join("frames").join(format!("frame_{frame:04}.notes.txt"));
    let mut note_body = String::from(
        "XCL frame record notes (same-process replay only; cross-process list rebuild unsupported)\n",
    );
    if settings().write_lists {
        note_body.push_str("- ListPayload: structured XLST + draw/dispatch bytes (not a full Cmd codec)\n");
    }
    for line in &notes {
        note_body.push_str(line);
        note_body.push('\n');
    }
    if notes.is_empty() && events.iter().any(|e| matches!(e, RecordedEvent::ListPayload { .. })) {
        note_body.push_str("- list payloads recorded; replay needs live COM pointers\n");
    }
    let _ = std::fs::write(&notes_path, note_body);
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

/// Pick PC command lists to re-submit: recorded segments, list-meta/payload ptrs, re-expand title wrappers, or backing.
fn resolve_replay_lists(
    addrs: &[usize],
    pc_addrs: &[usize],
    backing: &[usize],
    list_ptrs: &[usize],
) -> (Vec<*mut c_void>, &'static str) {
    if !pc_addrs.is_empty() && pc_addrs.iter().all(|&a| pointer_live(a)) {
        return (
            pc_addrs.iter().map(|&a| a as *mut c_void).collect(),
            "pc_addrs",
        );
    }
    if !list_ptrs.is_empty() && list_ptrs.iter().all(|&a| pointer_live(a)) {
        return (
            list_ptrs.iter().map(|&a| a as *mut c_void).collect(),
            "list_meta",
        );
    }
    let live_meta: Vec<*mut c_void> = list_ptrs
        .iter()
        .filter(|&&a| pointer_live(a))
        .map(|&a| a as *mut c_void)
        .collect();
    if !live_meta.is_empty() {
        return (live_meta, "list_meta_partial");
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
        return (expanded, "thin_expand");
    }
    let backing_live: Vec<*mut c_void> = backing
        .iter()
        .filter(|&&b| pointer_live(b))
        .map(|&b| b as *mut c_void)
        .collect();
    if !backing_live.is_empty() {
        return (backing_live, "backing");
    }
    (Vec::new(), "none")
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
            gpu_vas: gpu_vas.clone(),
            bound_heaps,
        };
        append_event(&ev);
        super::host::timeline::frame_record(
            "xcl_record_submit",
            format!("n={n} lists={} wire={}", title_lists.len(), settings().write_wire),
        );
        if settings().write_wire || settings().write_contents || settings().write_lists {
            for meta in list_meta_events(mapped) {
                append_event(&meta);
            }
        }
        if settings().write_lists {
            for payload in list_payload_events(mapped) {
                append_event(&payload);
            }
        }
        if settings().write_contents {
            for sample in content_sample_events(&gpu_vas) {
                append_event(&sample);
            }
        }
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

    let pending = PENDING_PRESENT.lock().unwrap().take();
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
            let (queue, planes) = pending
                .as_ref()
                .map(|p| (p.queue, p.planes.clone()))
                .unwrap_or((0, Vec::new()));
            let ev = RecordedEvent::Present {
                frame,
                token,
                submits: SUBMITS.load(Ordering::Relaxed),
                queue,
                planes,
            };
            append_event(&ev);
            super::host::timeline::frame_record("xcl_record_present", format!("frame={frame} token={token}"));
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

    if !settings().replay_execute && !settings().replay_present {
        crate::xlog_info!(
            "D3D12.X: XCL_REPLAY: frame {frame_idx} (token {token}): {} event(s) — execute off (XCL_REPLAY_EXECUTE=0)",
            events.len()
        );
        return;
    }

    let mut submits = 0u32;
    let mut signals = 0u32;
    let mut skipped = 0u32;
    let mut content_ok = 0u32;
    let mut content_bad = 0u32;
    let mut content_skip = 0u32;
    let mut presents = 0u32;
    let mut list_ptrs: Vec<usize> = Vec::new();
    for ev in events.iter() {
        match ev {
            RecordedEvent::ListMeta { list, .. } | RecordedEvent::ListPayload { list, .. } => {
                if *list != 0 && !list_ptrs.contains(list) {
                    list_ptrs.push(*list);
                }
            }
            _ => {}
        }
    }

    for ev in events.iter() {
        match ev {
            RecordedEvent::Submit {
                queue,
                addrs,
                pc_addrs,
                backing,
                ..
            } => {
                if !settings().replay_execute {
                    continue;
                }
                if !pointer_live(*queue) {
                    skipped += 1;
                    crate::xlog_info!(
                        "D3D12.X: XCL_REPLAY: submit skipped — queue {queue:#x} not live (cross-process / freed handle)"
                    );
                    continue;
                }
                let (live, how) = resolve_replay_lists(addrs, pc_addrs, backing, &list_ptrs);
                if live.is_empty() {
                    skipped += 1;
                    crate::xlog_info!(
                        "D3D12.X: XCL_REPLAY: submit skipped — no live lists (need same-process pointers; payload bytes alone cannot rebuild COM lists)"
                    );
                    continue;
                }
                unsafe {
                    super::title_textures::own_work(|| {
                        super::real::execute_mapped_lists(*queue as *mut c_void, &live);
                    });
                }
                submits += 1;
                crate::xlog_info!(
                    "D3D12.X: XCL_REPLAY: frame {frame_idx}: re-issued {} list(s) via {how}",
                    live.len()
                );
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
            RecordedEvent::Present {
                frame,
                token: t,
                queue,
                planes,
                ..
            } => {
                if settings().replay_present {
                    if *queue != 0 && pointer_live(*queue) && !planes.is_empty() {
                        let plane_refs: Vec<(usize, Option<[i32; 4]>)> = planes
                            .iter()
                            .filter(|p| pointer_live(p.resource))
                            .map(|p| (p.resource, p.rect))
                            .collect();
                        if !plane_refs.is_empty() {
                            unsafe {
                                super::present::reissue_present_x(
                                    *queue as *mut c_void,
                                    *t,
                                    &plane_refs,
                                );
                            }
                            presents += 1;
                            crate::xlog_info!(
                                "D3D12.X: XCL_REPLAY: PresentX re-issued frame idx {frame_idx} (recorded {frame}, token {t}, {} plane(s))",
                                plane_refs.len()
                            );
                        } else {
                            skipped += 1;
                            crate::xlog_info!(
                                "D3D12.X: XCL_REPLAY: PresentX not re-issued — plane resources not live (same-process only)"
                            );
                        }
                    } else {
                        skipped += 1;
                        crate::xlog_info!(
                            "D3D12.X: XCL_REPLAY: PresentX not re-issued — missing queue/planes or queue not live"
                        );
                    }
                } else {
                    crate::xlog_info!(
                        "D3D12.X: XCL_REPLAY: replayed frame idx {frame_idx} (recorded frame {frame}, token {t}); PresentX not re-issued (set XCL_REPLAY_PRESENT=1)"
                    );
                }
            }
            RecordedEvent::HeapSnap { heaps } => {
                crate::xlog_info!(
                    "D3D12.X: XCL_REPLAY: frame {frame_idx}: heap snap {} heap(s) (metadata only)",
                    heaps.len()
                );
            }
            RecordedEvent::ListMeta { list, list_kind, .. } => {
                crate::xlog_info!(
                    "D3D12.X: XCL_REPLAY: frame {frame_idx}: list meta ptr={list:#x} list_kind={list_kind} live={}",
                    pointer_live(*list)
                );
            }
            RecordedEvent::ListPayload { list, list_kind, ops } => {
                crate::xlog_info!(
                    "D3D12.X: XCL_REPLAY: frame {frame_idx}: list payload ptr={list:#x} list_kind={list_kind} ops={} live={}",
                    ops.len(),
                    pointer_live(*list)
                );
            }
            RecordedEvent::ContentSample { va, len, hash, .. }
            | RecordedEvent::ContentChunk { va, len, hash, .. } => {
                if !settings().replay_contents {
                    continue;
                }
                match verify_content_sample(*va, *len, *hash) {
                    Some(true) => content_ok += 1,
                    Some(false) => content_bad += 1,
                    None => content_skip += 1,
                }
            }
            RecordedEvent::Session { .. } => {}
        }
    }

    if submits > 0 || signals > 0 || skipped > 0 || presents > 0 || content_ok + content_bad + content_skip > 0 {
        crate::xlog_info!(
            "D3D12.X: XCL_REPLAY: frame {frame_idx}: {submits} submit(s), {signals} signal(s), {presents} present(s), {skipped} skipped; content ok={content_ok} mismatch={content_bad} unread={content_skip}"
        );
    }
}

/// Offline inspect for `xcl replay <dir>` (no GPU device).
pub fn cli_inspect(dir: &Path, json: bool, frame_filter: Option<u64>) -> Result<String, String> {
    let index = dir.join("index.jsonl");
    if !index.is_file() {
        return Err(format!("missing {}", index.display()));
    }
    let session = std::fs::read_to_string(dir.join("session.json")).unwrap_or_default();
    let mut frames = Vec::new();
    let mut n = 0u64;
    loop {
        n += 1;
        let bin = dir.join("frames").join(format!("frame_{n:04}.bin"));
        let jsonl = dir.join("frames").join(format!("frame_{n:04}.jsonl"));
        let contents = dir.join("frames").join(format!("frame_{n:04}.contents.bin"));
        if !bin.is_file() && !jsonl.is_file() {
            break;
        }
        if frame_filter.is_some_and(|f| f != n) {
            continue;
        }
        let (source, events) = if bin.is_file() {
            let wire = super::frame_wire::read_frame(&bin)?;
            ("bin", recorded_from_wire(&wire))
        } else {
            let ev = load_jsonl_frame(dir, n).ok_or_else(|| format!("empty frame {n}"))?;
            ("jsonl", ev)
        };
        let mut submits = 0u32;
        let mut list_meta = 0u32;
        let mut list_payload = 0u32;
        let mut samples = 0u32;
        let mut chunks = 0u32;
        let mut signals = 0u32;
        let mut planes = 0u32;
        let mut payload_bytes = 0usize;
        for ev in &events {
            match ev {
                RecordedEvent::Submit { .. } => submits += 1,
                RecordedEvent::ListMeta { .. } => list_meta += 1,
                RecordedEvent::ListPayload { ops, .. } => {
                    list_payload += 1;
                    payload_bytes += ops.len();
                }
                RecordedEvent::ContentSample { .. } => samples += 1,
                RecordedEvent::ContentChunk { .. } => chunks += 1,
                RecordedEvent::Signal { .. } => signals += 1,
                RecordedEvent::Present { planes: p, .. } => planes += p.len() as u32,
                _ => {}
            }
        }
        frames.push(serde_json::json!({
            "frame": n,
            "source": source,
            "events": events.len(),
            "submits": submits,
            "list_meta": list_meta,
            "list_payload": list_payload,
            "payload_bytes": payload_bytes,
            "content_samples": samples,
            "content_chunks": chunks,
            "signals": signals,
            "present_planes": planes,
            "contents_bin_bytes": contents.metadata().map(|m| m.len()).unwrap_or(0),
        }));
    }
    if frames.is_empty() {
        return Err("no frames/frame_*.bin or frame_*.jsonl found".into());
    }
    if json {
        Ok(serde_json::json!({
            "dir": dir.display().to_string(),
            "session": serde_json::from_str::<serde_json::Value>(&session).unwrap_or(serde_json::Value::Null),
            "wire_version_writer": super::frame_wire::VERSION,
            "cross_process": "unsupported",
            "frames": frames,
        })
        .to_string())
    } else {
        let mut out = format!(
            "xcl replay inspect {}\nwire writer v{} · {} frame(s)\ncross-process list rebuild: unsupported (COM handles are process-local)\n",
            dir.display(),
            super::frame_wire::VERSION,
            frames.len()
        );
        for f in &frames {
            out.push_str(&format!(
                "  frame {}: {} events, {} submit(s), {} listpayload ({} B ops), {} content chunk(s), contents.bin {} B, {} present plane(s)\n",
                f["frame"],
                f["events"],
                f["submits"],
                f["list_payload"],
                f["payload_bytes"],
                f["content_chunks"],
                f["contents_bin_bytes"],
                f["present_planes"],
            ));
        }
        Ok(out)
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
    fn wire_recorded_roundtrip_v4_fields() {
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
            RecordedEvent::ListMeta {
                list: 2,
                list_kind: 0,
                pso: 3,
                rs_gfx: 4,
                heaps: vec![0x80],
                vas: vec![0x5000],
            },
            RecordedEvent::ListPayload {
                list: 2,
                list_kind: 0,
                ops: vec![1, 2, 3],
            },
            RecordedEvent::ContentSample {
                va: 0x5000,
                len: 4,
                hash: super::super::frame_wire::fnv1a32(&[9, 8, 7, 6]),
                sample: vec![9, 8, 7, 6],
            },
            RecordedEvent::ContentChunk {
                va: 0x5000,
                len: 4,
                hash: super::super::frame_wire::fnv1a32(&[9, 8, 7, 6]),
                file_off: 0,
            },
            RecordedEvent::HeapSnap {
                heaps: vec![HeapEntry {
                    ptr: 0x80,
                    ty: 0,
                    count: 16,
                    flags: 1,
                    gpu_start: 0x1111,
                    cpu_start: 0x2222,
                }],
            },
            RecordedEvent::Present {
                frame: 1,
                token: 7,
                submits: 2,
                queue: 0x40,
                planes: vec![PresentPlaneRec {
                    resource: 0x99,
                    rect: Some([0, 0, 8, 8]),
                    width: 8,
                    height: 8,
                    format: 28,
                }],
            },
        ];
        let wire = events_to_wire(&recorded);
        let back = recorded_from_wire(&wire);
        assert_eq!(back, recorded);
        let bytes = super::super::frame_wire::encode_frame(&wire);
        let decoded = super::super::frame_wire::decode_frame(&bytes).unwrap();
        assert_eq!(recorded_from_wire(&decoded), recorded);
    }
}
