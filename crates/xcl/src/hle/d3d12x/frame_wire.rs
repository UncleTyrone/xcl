//! Binary frame wire (`XCLFRAME`) — GPL-2 stand-in for CGBJ `VICFRAME` capture blobs.
//!
//! Opt-in via `XCL_RECORD_WIRE=1` while `XCL_RECORD` is set: each recorded present also writes
//! `frames/frame_NNNN.bin` (magic + version + little-endian event stream). Not a byte-compatible
//! CGBJ `VICFRAME` file (no heap/PSO/resource snap); same operator role for submit/signal/present.

use std::path::Path;

/// First eight bytes of `frame_*.bin`.
pub const MAGIC: [u8; 8] = *b"XCLFRAME";
/// Layout version for this event stream.
pub const VERSION: u32 = 1;

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

/// Wire tags for [`super::frame_replay`] events (stable).
pub mod tag {
    pub const SESSION: u8 = 1;
    pub const SUBMIT: u8 = 2;
    pub const SIGNAL: u8 = 3;
    pub const PRESENT: u8 = 4;
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
}

impl WireEvent {
    pub fn put(&self, out: &mut Vec<u8>) {
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
            } => {
                out.push(tag::SUBMIT);
                put_u64(out, *n);
                put_u64(out, *queue as u64);
                put_u32(out, *lists as u32);
                put_usize_list(out, addrs);
                put_usize_list(out, pc_addrs);
                put_usize_list(out, backing);
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
        }
    }

    pub fn get(from: &mut Dec<'_>) -> Result<Self, Bad> {
        Ok(match *from.take(1)?.first().ok_or(Bad("tag"))? {
            tag::SESSION => Self::Session {
                format: get_u32(from)?,
            },
            tag::SUBMIT => Self::Submit {
                n: get_u64(from)?,
                queue: get_u64(from)? as usize,
                lists: get_u32(from)? as usize,
                addrs: get_usize_list(from)?,
                pc_addrs: get_usize_list(from)?,
                backing: get_usize_list(from)?,
            },
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
            _ => return Err(Bad("unknown event kind")),
        })
    }
}

/// Encode a frame blob: magic + version + events.
pub fn encode_frame(events: &[WireEvent]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    put_u32(&mut out, VERSION);
    put_u32(&mut out, events.len() as u32);
    for ev in events {
        ev.put(&mut out);
    }
    out
}

/// Decode a frame blob written by [`encode_frame`].
pub fn decode_frame(bytes: &[u8]) -> Result<Vec<WireEvent>, Bad> {
    let mut from = Dec::new(bytes);
    if from.take(8)? != MAGIC {
        return Err(Bad("not an XCLFRAME"));
    }
    if get_u32(&mut from)? != VERSION {
        return Err(Bad("unsupported version"));
    }
    let n = get_u32(&mut from)? as usize;
    if n > from.bytes.len() {
        return Err(Bad("event count too large"));
    }
    let mut events = Vec::with_capacity(n);
    for _ in 0..n {
        events.push(WireEvent::get(&mut from)?);
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
    fn roundtrip_submit_present() {
        let events = vec![
            WireEvent::Session { format: 1 },
            WireEvent::Submit {
                n: 3,
                queue: 0x1000,
                lists: 2,
                addrs: vec![1, 2],
                pc_addrs: vec![3],
                backing: vec![0],
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
}
