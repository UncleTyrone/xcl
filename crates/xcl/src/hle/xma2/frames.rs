//! XMA packet / frame bit positions (GPL-2; behavior aligned with console XMA2 layout).
//!
//! A 2048-byte packet starts with a 32-bit big-endian header: frame count (6),
//! first-frame bit offset (15), 3 reserved bits, skip count (8). Frames hold a
//! 15-bit length field then payload bits MSB-first.

use super::PACKET_SIZE;

pub const PACKET_BYTES: usize = PACKET_SIZE;
const PACKET_BITS: usize = PACKET_BYTES * 8;
const HEADER_BITS: usize = 32;
const LENGTH_BITS: u32 = 15;

/// Parsed XMA2 packet header (one interleaved stream).
#[derive(Debug, Clone, Copy)]
pub struct Packet {
    pub frames: u32,
    pub first_frame_bit: u32,
    pub skip: u32,
}

pub fn packet(data: &[u8], index: usize) -> Option<Packet> {
    let start = index.checked_mul(PACKET_BYTES)?;
    let b = data.get(start..)?.get(..4)?;
    let h = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    Some(Packet {
        frames: h >> 26,
        first_frame_bit: (h >> 11) & 0x7fff,
        skip: h & 0xff,
    })
}

/// One XMA frame extracted from a packet buffer (single stream).
#[derive(Debug, Clone)]
pub struct Frame {
    pub data: Vec<u8>,
    pub bits: usize,
    pub start: usize,
    pub next: Option<usize>,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn bit(&mut self) -> Option<u32> {
        if self.pos.is_multiple_of(PACKET_BITS) {
            let previous = self.pos / PACKET_BITS;
            if previous > 0 {
                let skip = packet(self.data, previous - 1)?.skip as usize;
                self.pos = (previous + skip) * PACKET_BITS;
            }
            self.pos += HEADER_BITS;
        }
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(u32::from(bit))
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0, |v, _| Some(v << 1 | self.bit()?))
    }

    fn byte(&mut self) -> Option<u8> {
        let in_packet = self.pos % PACKET_BITS;
        if in_packet == 0 || in_packet > PACKET_BITS - 8 {
            return self.bits(8).map(|v| v as u8);
        }
        let at = self.pos / 8;
        let shift = self.pos % 8;
        let first = *self.data.get(at)?;
        let byte = if shift == 0 {
            first
        } else {
            (first << shift) | (self.data.get(at + 1)? >> (8 - shift))
        };
        self.pos += 8;
        Some(byte)
    }
}

/// Frame starting at bit offset `start` inside a buffer of whole packets for one stream.
pub fn read_frame(data: &[u8], start: usize) -> Option<Frame> {
    let mut cursor = Cursor { data, pos: start };
    let bits = cursor.bits(LENGTH_BITS)? as usize;
    if bits <= LENGTH_BITS as usize || bits == 0x7fff {
        return None;
    }
    let mut cursor = Cursor { data, pos: start };
    let mut out = Vec::with_capacity(bits.div_ceil(8));
    for _ in 0..bits / 8 {
        out.push(cursor.byte()?);
    }
    let tail = (bits % 8) as u32;
    if tail != 0 {
        out.push((cursor.bits(tail)? << (8 - tail)) as u8);
    }
    let end = cursor.pos;
    let next = next_frame(data, start, end);
    Some(Frame {
        data: out,
        bits,
        start,
        next,
    })
}

fn next_frame(data: &[u8], start: usize, end: usize) -> Option<usize> {
    let index = start / PACKET_BITS;
    let header = packet(data, index)?;
    let mut at = index * PACKET_BITS + HEADER_BITS + header.first_frame_bit as usize;
    let mut number = 0;
    while at < start {
        let mut cursor = Cursor { data, pos: at };
        let bits = cursor.bits(LENGTH_BITS)? as usize;
        if bits <= LENGTH_BITS as usize {
            return None;
        }
        at += bits;
        number += 1;
    }
    let last = number + 1 >= header.frames;
    let mut end = end;
    let ends_in_packet_without_frames = end / PACKET_BITS != index
        && !end.is_multiple_of(PACKET_BITS)
        && packet(data, end / PACKET_BITS)?.frames == 0;
    if end.is_multiple_of(PACKET_BITS) || (last && end / PACKET_BITS == index) || ends_in_packet_without_frames {
        let mut next = (end - 1) / PACKET_BITS + 1 + packet(data, (end - 1) / PACKET_BITS)?.skip as usize;
        let mut header = packet(data, next)?;
        while header.frames == 0 {
            next += 1 + header.skip as usize;
            header = packet(data, next)?;
        }
        end = next * PACKET_BITS + HEADER_BITS + header.first_frame_bit as usize;
    }
    (end / 8 < data.len()).then_some(end)
}

/// Default first-frame bit when the context bit cursor is below the header (32).
pub fn default_frame_start(data: &[u8]) -> usize {
    32 + packet(data, 0).map_or(0, |p| p.first_frame_bit as usize)
}

/// Resolve a hardware bit cursor for seek: values below 32 become the first frame of packet 0;
/// otherwise the cursor is used as a mid-packet / cross-packet frame start.
pub fn seek_bit(data: &[u8], cursor: usize) -> usize {
    if cursor < HEADER_BITS {
        default_frame_start(data)
    } else {
        cursor
    }
}

/// Walk frames from `start` until `target` is reached or passed; returns the frame that
/// contains `target` when `target` falls inside a frame, else the frame beginning at `target`.
pub fn frame_covering(data: &[u8], start: usize, target: usize) -> Option<Frame> {
    let mut at = seek_bit(data, start);
    while let Some(frame) = read_frame(data, at) {
        if frame.start == target {
            return Some(frame);
        }
        if frame.start < target {
            if frame.next.is_none_or(|n| target < n) {
                // Target sits inside this frame's payload (loop-end bit mid-frame).
                return Some(frame);
            }
            at = frame.next?;
            continue;
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hle::xma2::tests::{pad_packet, MONO_48K_PREFIX};

    #[test]
    fn packet_header_and_frame_chain() {
        let data = MONO_48K_PREFIX;
        let p0 = packet(data, 0).expect("packet 0");
        assert!(p0.frames >= 1);
        let start = HEADER_BITS + p0.first_frame_bit as usize;
        let f = read_frame(data, start).expect("first frame");
        assert!(f.bits > LENGTH_BITS as usize);
        assert!(f.start == start);
        if let Some(n) = f.next {
            assert!(read_frame(data, n).is_some());
        }
    }

    #[test]
    fn mid_packet_seek_reads_second_frame() {
        let data = pad_packet(MONO_48K_PREFIX);
        let start = default_frame_start(&data);
        let first = read_frame(&data, start).expect("first");
        let second_at = first.next.expect("second frame exists");
        assert!(second_at > start);
        assert!(second_at > HEADER_BITS);
        let second = read_frame(&data, second_at).expect("mid-packet seek");
        assert_eq!(second.start, second_at);
        assert!(second.bits > LENGTH_BITS as usize);
        // Seek helper: cursor below header → first frame; exact bit → that frame.
        assert_eq!(seek_bit(&data, 0), start);
        assert_eq!(seek_bit(&data, second_at), second_at);
        let covered = frame_covering(&data, start, second_at).expect("cover");
        assert_eq!(covered.start, second_at);
    }

    #[test]
    fn seek_into_frame_payload_returns_containing_frame() {
        let data = pad_packet(MONO_48K_PREFIX);
        let start = default_frame_start(&data);
        let first = read_frame(&data, start).expect("first");
        let inside = first.start + (first.bits / 2).max(1);
        let covered = frame_covering(&data, start, inside).expect("inside first");
        assert_eq!(covered.start, first.start);
    }
}