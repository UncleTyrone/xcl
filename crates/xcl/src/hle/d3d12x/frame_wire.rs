//! Binary frame wire (`XCLFRAME`) — GPL-2 stand-in for CGBJ `VICFRAME` capture blobs.
//!
//! Opt-in via `XCL_RECORD_WIRE=1` while `XCL_RECORD` is set: each recorded present also writes
//! `frames/frame_NNNN.bin` (magic + version + little-endian event stream). Not a byte-compatible
//! CGBJ `VICFRAME` file; same operator role for submit/signal/present with a practical metadata
//! subset for validation (heaps, bind ids, title GPU addresses, list meta/payloads, title-memory
//! samples and content chunks).
//!
//! ## Cross-process replay (why it stays same-process)
//!
//! `ListPayload` command bytes and `ListMeta` pointer ids are enough to **diagnose** a frame
//! offline (`xcl replay --listing/--edit`) but not to rebuild live `ID3D12GraphicsCommandList`
//! objects in another process: COM interface pointers and title GPU VAs are tied to one virtual
//! address space and one D3D12 device lifetime. Windows does not expose a supported API to
//! “rehydrate” a recorded command list from bytes alone (unlike writing a new list from host code).
//! XCL therefore documents cross-process play as **unsupported** and re-issues only while the title
//! is live with still-readable pointers (`XCL_REPLAY` / `--run`).
//!
//! ## `XCLFRAME` version 5 (current [`VERSION`])
//!
//! Same stream as version 4, plus:
//! - **`HeapSnap` / `HeapEntry`** — optional GPU/CPU descriptor-heap start handles
//!   (VICFRAME `DescriptorHeap` id/kind/count role without full slot-run codecs).
//! - **`Present` plane tail** — per-plane `width` / `height` / `format` from `GetDesc` when known
//!   (VICFRAME `Present` picture meta). Texel bytes live in sidecar `frames/frame_NNNN.present.bin`
//!   (JSONL carries `pitch` / `file_off` / `hash`; wire v5 layout unchanged).
//!
//! ## `XCLFRAME` version 4
//!
//! Same stream as version 3, plus:
//! - **`ListPayload` (tag 8)** — structured command dump for a mapped PC list (bind snapshot,
//!   written ranges, and optional draw/dispatch op bytes). VICFRAME `List.commands` role without
//!   a full host `Cmd` codec; same-process re-issue still uses live list pointers.
//! - **`ContentChunk` (tag 9)** — title-memory region hashed and stored in sidecar
//!   `frames/frame_NNNN.contents.bin` at `file_off` (VICFRAME `contents.bin` / `Content` role).
//! - **`Present` (tag 4) tail** — when `wire_version >= 4`: `queue`, `plane_count`, and per-plane
//!   `(resource, optional source rect)` so replay can re-issue `PresentX` in-process.
//!
//! ## `XCLFRAME` version 3
//!
//! - **`ListMeta` (tag 6)** — per mapped PC list: pointer, `D3D12_COMMAND_LIST_TYPE`, PSO/RS ids,
//!   bound heap ptrs, and title GPU VAs touched on that list.
//! - **`ContentSample` (tag 7)** — title-memory sample at a GPU VA: length hashed, FNV-1a-32,
//!   and a leading sample prefix.
//!
//! ## Version 2
//!
//! - **`Submit` (tag 2)** — after the v1 `backing` list: `psos`, `rs_gfx`, `gpu_vas` (u64),
//!   `bound_heaps` (usize), each a length-prefixed list (little-endian).
//! - **`HeapSnap` (tag 5)** — per present: length-prefixed `HeapEntry` records
//!   `(ptr u64, type u32, count u32, flags u32)` for descriptor heaps known this frame.
//!
//! Version 1 blobs omit the `Submit` tail and have no `HeapSnap` events. [`decode_frame`] accepts
//! versions 1..=5.

use std::path::Path;

/// First eight bytes of `frame_*.bin`.
pub const MAGIC: [u8; 8] = *b"XCLFRAME";
/// Layout version for newly written blobs (see module docs for field differences vs 1..=4).
pub const VERSION: u32 = 5;
pub const VERSION_MIN: u32 = 1;

/// Max leading bytes stored in a [`WireEvent::ContentSample`].
pub const CONTENT_SAMPLE_CAP: usize = 4096;

/// Max bytes hashed / dumped per title VA into `contents.bin` (one chunk).
pub const CONTENT_CHUNK_CAP: usize = 256 * 1024;

/// Soft cap on total `contents.bin` bytes written for one frame.
pub const CONTENTS_BIN_SOFT_CAP: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bad(pub &'static str);

impl std::fmt::Display for Bad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "xcl frame wire: {}", self.0)
    }
}

impl std::error::Error for Bad {}

pub struct Dec<'a> {
    pub bytes: &'a [u8],
}

impl<'a> Dec<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Bad> {
        if self.bytes.len() < n {
            return Err(Bad("truncated"));
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn get_u32(from: &mut Dec<'_>) -> Result<u32, Bad> {
    let b = from.take(4)?;
    Ok(u32::from_le_bytes(b.try_into().map_err(|_| Bad("u32"))?))
}

fn get_u64(from: &mut Dec<'_>) -> Result<u64, Bad> {
    let b = from.take(8)?;
    Ok(u64::from_le_bytes(b.try_into().map_err(|_| Bad("u64"))?))
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn get_i32(from: &mut Dec<'_>) -> Result<i32, Bad> {
    let b = from.take(4)?;
    Ok(i32::from_le_bytes(b.try_into().map_err(|_| Bad("i32"))?))
}

fn put_usize_list(out: &mut Vec<u8>, xs: &[usize]) {
    put_u32(out, xs.len() as u32);
    for &x in xs {
        put_u64(out, x as u64);
    }
}

fn get_usize_list(from: &mut Dec<'_>) -> Result<Vec<usize>, Bad> {
    let n = get_u32(from)? as usize;
    if n > from.bytes.len() {
        return Err(Bad("list longer than remaining"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(get_u64(from)? as usize);
    }
    Ok(out)
}

fn put_u64_list(out: &mut Vec<u8>, xs: &[u64]) {
    put_u32(out, xs.len() as u32);
    for &x in xs {
        put_u64(out, x);
    }
}

fn get_u64_list(from: &mut Dec<'_>) -> Result<Vec<u64>, Bad> {
    let n = get_u32(from)? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(get_u64(from)?);
    }
    Ok(out)
}

fn put_bytes(out: &mut Vec<u8>, xs: &[u8]) {
    put_u32(out, xs.len() as u32);
    out.extend_from_slice(xs);
}

fn get_bytes(from: &mut Dec<'_>) -> Result<Vec<u8>, Bad> {
    let n = get_u32(from)? as usize;
    if n > from.bytes.len() {
        return Err(Bad("bytes longer than remaining"));
    }
    Ok(from.take(n)?.to_vec())
}

/// FNV-1a 32-bit (stable, GPL-2, no extra crate) for content digests.
pub fn fnv1a32(data: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5_u32;
    for &b in data {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Wire tags for [`super::frame_replay`] events (stable).
pub mod tag {
    pub const SESSION: u8 = 1;
    pub const SUBMIT: u8 = 2;
    pub const SIGNAL: u8 = 3;
    pub const PRESENT: u8 = 4;
    pub const HEAP_SNAP: u8 = 5;
    pub const LIST_META: u8 = 6;
    pub const CONTENT_SAMPLE: u8 = 7;
    pub const LIST_PAYLOAD: u8 = 8;
    pub const CONTENT_CHUNK: u8 = 9;
}

/// Descriptor heap metadata (pointer id + creation sizes; no descriptor bytes).
///
/// v5 adds GPU/CPU heap-start handles (0 when unknown) — VICFRAME `DescriptorHeap` spine without
/// slot-run codecs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeapEntry {
    pub ptr: usize,
    pub ty: u32,
    pub count: u32,
    pub flags: u32,
    /// `GetGPUDescriptorHandleForHeapStart` (v5+; 0 on older wire / unknown).
    pub gpu_start: u64,
    /// `GetCPUDescriptorHandleForHeapStart` (v5+; 0 on older wire / unknown).
    pub cpu_start: u64,
}

impl HeapEntry {
    fn put(&self, out: &mut Vec<u8>, wire_version: u32) {
        put_u64(out, self.ptr as u64);
        put_u32(out, self.ty);
        put_u32(out, self.count);
        put_u32(out, self.flags);
        if wire_version >= 5 {
            put_u64(out, self.gpu_start);
            put_u64(out, self.cpu_start);
        }
    }

    fn get(from: &mut Dec<'_>, wire_version: u32) -> Result<Self, Bad> {
        let ptr = get_u64(from)? as usize;
        let ty = get_u32(from)?;
        let count = get_u32(from)?;
        let flags = get_u32(from)?;
        let (gpu_start, cpu_start) = if wire_version >= 5 {
            (get_u64(from)?, get_u64(from)?)
        } else {
            (0, 0)
        };
        Ok(Self {
            ptr,
            ty,
            count,
            flags,
            gpu_start,
            cpu_start,
        })
    }
}

/// One PresentX plane resource (+ optional source rect) for v4+ present tails.
///
/// v5 adds resource `GetDesc` width/height/format (0 when unknown) toward VICFRAME `Present`
/// picture metadata. Texel dump is a sidecar (`present.bin`), not part of the v5 plane tail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentPlane {
    pub resource: usize,
    pub rect: Option<[i32; 4]>,
    pub width: u32,
    pub height: u32,
    pub format: u32,
}

impl PresentPlane {
    fn put(&self, out: &mut Vec<u8>, wire_version: u32) {
        put_u64(out, self.resource as u64);
        match self.rect {
            Some(r) => {
                out.push(1);
                for v in r {
                    put_i32(out, v);
                }
            }
            None => out.push(0),
        }
        if wire_version >= 5 {
            put_u32(out, self.width);
            put_u32(out, self.height);
            put_u32(out, self.format);
        }
    }

    fn get(from: &mut Dec<'_>, wire_version: u32) -> Result<Self, Bad> {
        let resource = get_u64(from)? as usize;
        let has = *from.take(1)?.first().ok_or(Bad("plane rect flag"))?;
        let rect = if has != 0 {
            Some([get_i32(from)?, get_i32(from)?, get_i32(from)?, get_i32(from)?])
        } else {
            None
        };
        let (width, height, format) = if wire_version >= 5 {
            (get_u32(from)?, get_u32(from)?, get_u32(from)?)
        } else {
            (0, 0, 0)
        };
        Ok(Self {
            resource,
            rect,
            width,
            height,
            format,
        })
    }
}

/// One recorded event in binary form (mirrors JSONL kinds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireEvent {
    Session { format: u32 },
    Submit {
        n: u64,
        queue: usize,
        lists: usize,
        addrs: Vec<usize>,
        pc_addrs: Vec<usize>,
        backing: Vec<usize>,
        /// v2: parallel to `pc_addrs` (or empty in v1).
        psos: Vec<usize>,
        rs_gfx: Vec<usize>,
        gpu_vas: Vec<u64>,
        bound_heaps: Vec<usize>,
    },
    Signal {
        queue: usize,
        fence: usize,
        value: u64,
    },
    Present {
        frame: u64,
        token: u64,
        submits: u64,
        /// v4+: queue that issued PresentX (0 if unknown / older wire).
        queue: usize,
        /// v4+: plane resources for PresentX re-issue.
        planes: Vec<PresentPlane>,
    },
    HeapSnap { heaps: Vec<HeapEntry> },
    /// v3: one mapped PC list's bind/VA snapshot (no command-byte blob).
    ListMeta {
        list: usize,
        kind: u32,
        pso: usize,
        rs_gfx: usize,
        heaps: Vec<usize>,
        vas: Vec<u64>,
    },
    /// v3: title-memory sample at a GPU VA (hash over `len` bytes; `sample` is a prefix).
    ContentSample {
        va: u64,
        len: u32,
        hash: u32,
        sample: Vec<u8>,
    },
    /// v4: structured list payload (op bytes) for same-process diagnosis / re-issue hints.
    ListPayload {
        list: usize,
        kind: u32,
        ops: Vec<u8>,
    },
    /// v4: content chunk stored in `contents.bin` at `file_off` (`len` bytes, `hash` of those).
    ContentChunk {
        va: u64,
        len: u32,
        hash: u32,
        file_off: u64,
    },
}

impl WireEvent {
    pub fn put(&self, out: &mut Vec<u8>, wire_version: u32) {
        match self {
            Self::Session { format } => {
                out.push(tag::SESSION);
                put_u32(out, *format);
            }
            Self::Submit {
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
            } => {
                out.push(tag::SUBMIT);
                put_u64(out, *n);
                put_u64(out, *queue as u64);
                put_u32(out, *lists as u32);
                put_usize_list(out, addrs);
                put_usize_list(out, pc_addrs);
                put_usize_list(out, backing);
                if wire_version >= 2 {
                    put_usize_list(out, psos);
                    put_usize_list(out, rs_gfx);
                    put_u64_list(out, gpu_vas);
                    put_usize_list(out, bound_heaps);
                }
            }
            Self::Signal { queue, fence, value } => {
                out.push(tag::SIGNAL);
                put_u64(out, *queue as u64);
                put_u64(out, *fence as u64);
                put_u64(out, *value);
            }
            Self::Present {
                frame,
                token,
                submits,
                queue,
                planes,
            } => {
                out.push(tag::PRESENT);
                put_u64(out, *frame);
                put_u64(out, *token);
                put_u64(out, *submits);
                if wire_version >= 4 {
                    put_u64(out, *queue as u64);
                    put_u32(out, planes.len() as u32);
                    for p in planes {
                        p.put(out, wire_version);
                    }
                }
            }
            Self::HeapSnap { heaps } => {
                out.push(tag::HEAP_SNAP);
                put_u32(out, heaps.len() as u32);
                for h in heaps {
                    h.put(out, wire_version);
                }
            }
            Self::ListMeta {
                list,
                kind,
                pso,
                rs_gfx,
                heaps,
                vas,
            } => {
                out.push(tag::LIST_META);
                put_u64(out, *list as u64);
                put_u32(out, *kind);
                put_u64(out, *pso as u64);
                put_u64(out, *rs_gfx as u64);
                put_usize_list(out, heaps);
                put_u64_list(out, vas);
            }
            Self::ContentSample { va, len, hash, sample } => {
                out.push(tag::CONTENT_SAMPLE);
                put_u64(out, *va);
                put_u32(out, *len);
                put_u32(out, *hash);
                put_bytes(out, sample);
            }
            Self::ListPayload { list, kind, ops } => {
                out.push(tag::LIST_PAYLOAD);
                put_u64(out, *list as u64);
                put_u32(out, *kind);
                put_bytes(out, ops);
            }
            Self::ContentChunk {
                va,
                len,
                hash,
                file_off,
            } => {
                out.push(tag::CONTENT_CHUNK);
                put_u64(out, *va);
                put_u32(out, *len);
                put_u32(out, *hash);
                put_u64(out, *file_off);
            }
        }
    }

    pub fn get(from: &mut Dec<'_>, wire_version: u32) -> Result<Self, Bad> {
        Ok(match *from.take(1)?.first().ok_or(Bad("tag"))? {
            tag::SESSION => Self::Session {
                format: get_u32(from)?,
            },
            tag::SUBMIT => {
                let mut ev = Self::Submit {
                    n: get_u64(from)?,
                    queue: get_u64(from)? as usize,
                    lists: get_u32(from)? as usize,
                    addrs: get_usize_list(from)?,
                    pc_addrs: get_usize_list(from)?,
                    backing: get_usize_list(from)?,
                    psos: Vec::new(),
                    rs_gfx: Vec::new(),
                    gpu_vas: Vec::new(),
                    bound_heaps: Vec::new(),
                };
                if wire_version >= 2 {
                    if let Self::Submit {
                        ref mut psos,
                        ref mut rs_gfx,
                        ref mut gpu_vas,
                        ref mut bound_heaps,
                        ..
                    } = ev
                    {
                        *psos = get_usize_list(from)?;
                        *rs_gfx = get_usize_list(from)?;
                        *gpu_vas = get_u64_list(from)?;
                        *bound_heaps = get_usize_list(from)?;
                    }
                }
                ev
            }
            tag::SIGNAL => Self::Signal {
                queue: get_u64(from)? as usize,
                fence: get_u64(from)? as usize,
                value: get_u64(from)?,
            },
            tag::PRESENT => {
                let frame = get_u64(from)?;
                let token = get_u64(from)?;
                let submits = get_u64(from)?;
                let (queue, planes) = if wire_version >= 4 {
                    let queue = get_u64(from)? as usize;
                    let n = get_u32(from)? as usize;
                    let mut planes = Vec::with_capacity(n);
                    for _ in 0..n {
                        planes.push(PresentPlane::get(from, wire_version)?);
                    }
                    (queue, planes)
                } else {
                    (0, Vec::new())
                };
                Self::Present {
                    frame,
                    token,
                    submits,
                    queue,
                    planes,
                }
            }
            tag::HEAP_SNAP => {
                let n = get_u32(from)? as usize;
                let mut heaps = Vec::with_capacity(n);
                for _ in 0..n {
                    heaps.push(HeapEntry::get(from, wire_version)?);
                }
                Self::HeapSnap { heaps }
            }
            tag::LIST_META => {
                if wire_version < 3 {
                    return Err(Bad("ListMeta requires wire version >= 3"));
                }
                Self::ListMeta {
                    list: get_u64(from)? as usize,
                    kind: get_u32(from)?,
                    pso: get_u64(from)? as usize,
                    rs_gfx: get_u64(from)? as usize,
                    heaps: get_usize_list(from)?,
                    vas: get_u64_list(from)?,
                }
            }
            tag::CONTENT_SAMPLE => {
                if wire_version < 3 {
                    return Err(Bad("ContentSample requires wire version >= 3"));
                }
                Self::ContentSample {
                    va: get_u64(from)?,
                    len: get_u32(from)?,
                    hash: get_u32(from)?,
                    sample: get_bytes(from)?,
                }
            }
            tag::LIST_PAYLOAD => {
                if wire_version < 4 {
                    return Err(Bad("ListPayload requires wire version >= 4"));
                }
                Self::ListPayload {
                    list: get_u64(from)? as usize,
                    kind: get_u32(from)?,
                    ops: get_bytes(from)?,
                }
            }
            tag::CONTENT_CHUNK => {
                if wire_version < 4 {
                    return Err(Bad("ContentChunk requires wire version >= 4"));
                }
                Self::ContentChunk {
                    va: get_u64(from)?,
                    len: get_u32(from)?,
                    hash: get_u32(from)?,
                    file_off: get_u64(from)?,
                }
            }
            _ => return Err(Bad("unknown event kind")),
        })
    }
}

/// Encode a frame blob: magic + version + events.
pub fn encode_frame(events: &[WireEvent]) -> Vec<u8> {
    encode_frame_version(events, VERSION)
}

/// Encode with an explicit wire version (tests).
pub fn encode_frame_version(events: &[WireEvent], wire_version: u32) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    put_u32(&mut out, wire_version);
    put_u32(&mut out, events.len() as u32);
    for ev in events {
        ev.put(&mut out, wire_version);
    }
    out
}

/// Decode a frame blob written by [`encode_frame`].
pub fn decode_frame(bytes: &[u8]) -> Result<Vec<WireEvent>, Bad> {
    let mut from = Dec::new(bytes);
    if from.take(8)? != MAGIC {
        return Err(Bad("not an XCLFRAME"));
    }
    let ver = get_u32(&mut from)?;
    if ver < VERSION_MIN || ver > VERSION {
        return Err(Bad("unsupported version"));
    }
    let n = get_u32(&mut from)? as usize;
    if n > from.bytes.len() {
        return Err(Bad("event count too large"));
    }
    let mut events = Vec::with_capacity(n);
    for _ in 0..n {
        events.push(WireEvent::get(&mut from, ver)?);
    }
    if !from.bytes.is_empty() {
        return Err(Bad("trailing bytes"));
    }
    Ok(events)
}

/// Write `path` as an XCLFRAME blob.
pub fn write_frame(path: &Path, events: &[WireEvent]) -> std::io::Result<()> {
    std::fs::write(path, encode_frame(events))
}

/// Read an XCLFRAME blob from `path`.
pub fn read_frame(path: &Path) -> Result<Vec<WireEvent>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    decode_frame(&bytes).map_err(|e| e.to_string())
}

/// Compact kind tags inside a [`WireEvent::ListPayload`] `ops` blob.
///
/// Not a byte-compatible CGBJ `Cmd` stream: draw/dispatch/indirect/mesh tags cover the live
/// ring XCL can observe; barriers/roots/views stay in the XLST bind/VA snap only.
pub mod list_op {
    pub const DRAW: u8 = 1;
    pub const DISPATCH: u8 = 2;
    #[allow(dead_code)]
    pub const SET_PSO: u8 = 3;
    pub const HEAPS: u8 = 4;
    pub const TOUCH_VA: u8 = 5;
    pub const WRITTEN: u8 = 6;
    pub const BIND_IDS: u8 = 7;
    /// `D3D12_COMMAND_LIST_TYPE` for the mapped list (u32 LE).
    pub const LIST_KIND: u8 = 8;
    /// ExecuteIndirect spine: sig, max, args ptr, args_off, count ptr, count_off.
    pub const INDIRECT: u8 = 9;
    /// DispatchMesh group counts (x,y,z).
    pub const DISPATCH_MESH: u8 = 10;
    /// Draw with `first_instance` (VICFRAME Draw / DrawIndexed completeness).
    pub const DRAW_EX: u8 = 11;
    pub const SNAP_MAGIC: &[u8; 4] = b"XLST";
    pub const SNAP_VER: u8 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_submit_present_v4() {
        let events = vec![
            WireEvent::Session { format: 4 },
            WireEvent::Submit {
                n: 3,
                queue: 0x1000,
                lists: 2,
                addrs: vec![1, 2],
                pc_addrs: vec![3],
                backing: vec![0],
                psos: vec![0xA],
                rs_gfx: vec![0xB],
                gpu_vas: vec![0x10_00, 0x20_00],
                bound_heaps: vec![0xC],
            },
            WireEvent::ListMeta {
                list: 3,
                kind: 0,
                pso: 0xA,
                rs_gfx: 0xB,
                heaps: vec![0xC],
                vas: vec![0x10_00],
            },
            WireEvent::ListPayload {
                list: 3,
                kind: 0,
                ops: vec![list_op::DRAW, 1, 0, 0, 0, 3],
            },
            WireEvent::ContentSample {
                va: 0x10_00,
                len: 4,
                hash: fnv1a32(&[1, 2, 3, 4]),
                sample: vec![1, 2, 3, 4],
            },
            WireEvent::ContentChunk {
                va: 0x10_00,
                len: 4,
                hash: fnv1a32(&[1, 2, 3, 4]),
                file_off: 0,
            },
            WireEvent::HeapSnap {
                heaps: vec![HeapEntry {
                    ptr: 0xC,
                    ty: 0,
                    count: 64,
                    flags: 1,
                    gpu_start: 0x1000,
                    cpu_start: 0x2000,
                }],
            },
            WireEvent::Signal {
                queue: 0x1000,
                fence: 0x2000,
                value: 9,
            },
            WireEvent::Present {
                frame: 1,
                token: 42,
                submits: 3,
                queue: 0x1000,
                planes: vec![PresentPlane {
                    resource: 0xABCD,
                    rect: Some([0, 0, 1920, 1080]),
                    width: 1920,
                    height: 1080,
                    format: 28,
                }],
            },
        ];
        let bytes = encode_frame(&events);
        assert_eq!(&bytes[..8], &MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), VERSION);
        let back = decode_frame(&bytes).unwrap();
        assert_eq!(back, events);
    }

    #[test]
    fn decodes_v1_submit_without_tail() {
        let events_v1 = vec![
            WireEvent::Session { format: 1 },
            WireEvent::Submit {
                n: 1,
                queue: 9,
                lists: 1,
                addrs: vec![1],
                pc_addrs: vec![2],
                backing: vec![2],
                psos: vec![],
                rs_gfx: vec![],
                gpu_vas: vec![],
                bound_heaps: vec![],
            },
            WireEvent::Present {
                frame: 1,
                token: 1,
                submits: 1,
                queue: 0,
                planes: vec![],
            },
        ];
        let bytes = encode_frame_version(&events_v1, 1);
        let back = decode_frame(&bytes).unwrap();
        assert_eq!(back, events_v1);
    }

    #[test]
    fn decodes_v3_present_without_planes() {
        let events = vec![WireEvent::Present {
            frame: 2,
            token: 9,
            submits: 4,
            queue: 0,
            planes: vec![],
        }];
        let bytes = encode_frame_version(&events, 3);
        let back = decode_frame(&bytes).unwrap();
        assert_eq!(back, events);
    }

    #[test]
    fn heap_snap_roundtrip() {
        let ev = WireEvent::HeapSnap {
            heaps: vec![
                HeapEntry {
                    ptr: 0x100,
                    ty: 1,
                    count: 8,
                    flags: 0,
                    gpu_start: 0x11,
                    cpu_start: 0x22,
                },
                HeapEntry {
                    ptr: 0x200,
                    ty: 0,
                    count: 2048,
                    flags: 2,
                    gpu_start: 0,
                    cpu_start: 0,
                },
            ],
        };
        let bytes = encode_frame(&[ev.clone()]);
        let back = decode_frame(&bytes).unwrap();
        assert_eq!(back, vec![ev]);
    }

    #[test]
    fn decodes_v4_heap_and_plane_without_v5_tails() {
        let events = vec![
            WireEvent::HeapSnap {
                heaps: vec![HeapEntry {
                    ptr: 1,
                    ty: 0,
                    count: 4,
                    flags: 0,
                    gpu_start: 0,
                    cpu_start: 0,
                }],
            },
            WireEvent::Present {
                frame: 1,
                token: 2,
                submits: 1,
                queue: 3,
                planes: vec![PresentPlane {
                    resource: 9,
                    rect: None,
                    width: 0,
                    height: 0,
                    format: 0,
                }],
            },
        ];
        let bytes = encode_frame_version(&events, 4);
        let back = decode_frame(&bytes).unwrap();
        assert_eq!(back, events);
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut bad = b"VICFRAME".to_vec();
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.extend_from_slice(&0u32.to_le_bytes());
        assert!(decode_frame(&bad).is_err());
    }

    #[test]
    fn fnv_stable() {
        assert_eq!(fnv1a32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
    }
}
