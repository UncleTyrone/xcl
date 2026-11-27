//! Binary frame wire (`XCLFRAME`) — GPL-2 stand-in for CGBJ `VICFRAME` capture blobs.
//!
//! Opt-in via `XCL_RECORD_WIRE=1` while `XCL_RECORD` is set: each recorded present also writes
//! `frames/frame_NNNN.bin` (magic + version + little-endian event stream). Not a byte-compatible
//! CGBJ `VICFRAME` file; same operator role for submit/signal/present with a practical metadata
//! subset for validation (heaps, bind ids, title GPU addresses, list meta, title-memory samples).
//!
//! ## `XCLFRAME` version 3 (current [`VERSION`])
//!
//! Same stream as version 2, plus:
//! - **`ListMeta` (tag 6)** — per mapped PC list: pointer, `D3D12_COMMAND_LIST_TYPE`, PSO/RS ids,
//!   bound heap ptrs, and title GPU VAs touched on that list (VICFRAME `List` role without
//!   recording host command bytes).
//! - **`ContentSample` (tag 7)** — title-memory sample at a GPU VA: length hashed, FNV-1a-32,
//!   and up to 256 leading bytes (VICFRAME `Content` / `contents.bin` role for same-state boot
//!   debugging; not a full resource dump).
//!
//! ## Version 2
//!
//! - **`Submit` (tag 2)** — after the v1 `backing` list: `psos`, `rs_gfx`, `gpu_vas` (u64),
//!   `bound_heaps` (usize), each a length-prefixed list (little-endian).
//! - **`HeapSnap` (tag 5)** — per present: length-prefixed `HeapEntry` records
//!   `(ptr u64, type u32, count u32, flags u32)` for descriptor heaps known this frame.
//!
//! Version 1 blobs omit the `Submit` tail and have no `HeapSnap` events. [`decode_frame`] accepts
//! versions 1..=3.

use std::path::Path;

/// First eight bytes of `frame_*.bin`.
pub const MAGIC: [u8; 8] = *b"XCLFRAME";
/// Layout version for newly written blobs (see module docs for field differences vs 1/2).
pub const VERSION: u32 = 3;
pub const VERSION_MIN: u32 = 1;

/// Max leading bytes stored in a [`WireEvent::ContentSample`].
pub const CONTENT_SAMPLE_CAP: usize = 256;

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
}

/// Descriptor heap metadata (pointer id + creation sizes; no descriptor bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeapEntry {
    pub ptr: usize,
    pub ty: u32,
    pub count: u32,
    pub flags: u32,
}

impl HeapEntry {
    fn put(&self, out: &mut Vec<u8>) {
        put_u64(out, self.ptr as u64);
        put_u32(out, self.ty);
        put_u32(out, self.count);
        put_u32(out, self.flags);
    }

    fn get(from: &mut Dec<'_>) -> Result<Self, Bad> {
        Ok(Self {
            ptr: get_u64(from)? as usize,
            ty: get_u32(from)?,
            count: get_u32(from)?,
            flags: get_u32(from)?,
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
            Self::Present { frame, token, submits } => {
                out.push(tag::PRESENT);
                put_u64(out, *frame);
                put_u64(out, *token);
                put_u64(out, *submits);
            }
            Self::HeapSnap { heaps } => {
                out.push(tag::HEAP_SNAP);
                put_u32(out, heaps.len() as u32);
                for h in heaps {
                    h.put(out);
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
            tag::PRESENT => Self::Present {
                frame: get_u64(from)?,
                token: get_u64(from)?,
                submits: get_u64(from)?,
            },
            tag::HEAP_SNAP => {
                let n = get_u32(from)? as usize;
                let mut heaps = Vec::with_capacity(n);
                for _ in 0..n {
                    heaps.push(HeapEntry::get(from)?);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_submit_present_v3() {
        let events = vec![
            WireEvent::Session { format: 3 },
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
            WireEvent::ContentSample {
                va: 0x10_00,
                len: 4,
                hash: fnv1a32(&[1, 2, 3, 4]),
                sample: vec![1, 2, 3, 4],
            },
            WireEvent::HeapSnap {
                heaps: vec![HeapEntry {
                    ptr: 0xC,
                    ty: 0,
                    count: 64,
                    flags: 1,
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
        ];
        let bytes = encode_frame_version(&events_v1, 1);
        let back = decode_frame(&bytes).unwrap();
        assert_eq!(back, events_v1);
    }

    #[test]
    fn decodes_v2_without_list_meta() {
        let events = vec![
            WireEvent::Submit {
                n: 1,
                queue: 1,
                lists: 1,
                addrs: vec![1],
                pc_addrs: vec![2],
                backing: vec![2],
                psos: vec![3],
                rs_gfx: vec![4],
                gpu_vas: vec![5],
                bound_heaps: vec![6],
            },
            WireEvent::HeapSnap {
                heaps: vec![HeapEntry {
                    ptr: 6,
                    ty: 0,
                    count: 8,
                    flags: 0,
                }],
            },
        ];
        let bytes = encode_frame_version(&events, 2);
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
                },
                HeapEntry {
                    ptr: 0x200,
                    ty: 0,
                    count: 2048,
                    flags: 2,
                },
            ],
        };
        let bytes = encode_frame(&[ev.clone()]);
        let back = decode_frame(&bytes).unwrap();
        assert_eq!(back, vec![ev]);
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
