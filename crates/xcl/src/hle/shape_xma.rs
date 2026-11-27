//! ACPHAL SHAPE ↔ XMA path (CGBJ `audio.rs` XMA contexts).
//!
//! Six SHAPE kinds are allocated in [`crate::hle::acphal`]; XMA contexts live at
//! **kind index 4**. Commands
//! `ENABLE_XMA` / `DISABLE_XMA` / `ENABLE_XMAS` / `DISABLE_XMAS` /
//! `ADVANCE_XMA_READ` on the AcpHal channel drive a decoder thread that fills
//! each enabled context's 256-byte PCM output ring from its input packets via
//! [`crate::hle::xma2::Decoder`].
//!
//! Frame-at-a-time decode via [`crate::hle::xma2::frames::read_frame`] +
//! [`crate::hle::xma2::Decoder::decode_frame_exact`] (bit-exact mid-packet seek).
//! Loop boundaries reset the bit cursor from context word 3; IMDCT overlap is carried across
//! frames on each voice decoder (word 8 hardware overlap buffer is unused, matching CGBJ).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use crate::hle::xma2::{self, Decoder, StreamConfig};

const ENABLE_XMA: u32 = 5;
const ENABLE_XMAS: u32 = 6;
const DISABLE_XMA: u32 = 7;
const DISABLE_XMAS: u32 = 8;
const ADVANCE_XMA_READ: u32 = 18;

const BLOCK_BYTES: usize = 256;
const SAMPLES_PER_BLOCK: usize = BLOCK_BYTES / 2;
const CONTEXT_WORDS: usize = 16;
/// Samples per XMA subframe at 48 kHz (512 / 4); used for start and loop skip fields.
const SUBFRAME_SAMPLES: usize = 128;
/// Error status when both inputs are invalid (hardware status 4).
const INPUT_INVALID: u32 = 4;
/// Kind index in XCL's 5-slot SHAPE table that holds XMA contexts (matches CGBJ index 4).
pub const XMA_KIND: usize = 4;

struct Voice {
    decoder: Option<Decoder>,
    pcm: VecDeque<i16>,
    /// Samples per channel left to drop (stream start or loop-edge trim).
    skip: usize,
    ended: bool,
}

struct Xma {
    array: usize,
    voices: Vec<Mutex<Option<Voice>>>,
}

impl Xma {
    const EMPTY: Xma = Xma {
        array: 0,
        voices: Vec::new(),
    };
}

static XMA: RwLock<Xma> = RwLock::new(Xma::EMPTY);

fn xma_read() -> std::sync::RwLockReadGuard<'static, Xma> {
    XMA.read().unwrap_or_else(|e| e.into_inner())
}

/// Bind the XMA decoder to SHAPE kind-4 contexts after allocation (6-kind table; XMA slot 4).
pub fn on_shape_allocated(arrays: &[usize; 6], counts: &[u32; 6]) {
    let mut xma = XMA.write().unwrap_or_else(|e| e.into_inner());
    xma.array = arrays.get(XMA_KIND).copied().unwrap_or(0);
    let n = counts.get(XMA_KIND).copied().unwrap_or(0) as usize;
    xma.voices = (0..n).map(|_| Mutex::new(None)).collect();
    if n > 0 {
        crate::xlog_info!("acphal: SHAPE XMA path armed ({n} context(s) at kind {XMA_KIND})");
    }
}

/// Drop all voices when SHAPE contexts are released.
pub fn on_shape_released() {
    *XMA.write().unwrap_or_else(|e| e.into_inner()) = Xma::EMPTY;
}

#[derive(Clone, Copy)]
struct Context(*mut u32);

impl Context {
    fn get(self, word: usize) -> u32 {
        // SAFETY: inside the context array the title shares with hardware.
        unsafe { self.0.add(word).read_volatile() }
    }

    fn set(self, word: usize, value: u32) {
        unsafe { self.0.add(word).write_volatile(value) }
    }

    fn field(self, word: usize, shift: u32, bits: u32) -> u32 {
        (self.get(word) >> shift) & ((1 << bits) - 1)
    }

    fn set_field(self, word: usize, shift: u32, bits: u32, value: u32) {
        let mask = ((1u32 << bits) - 1) << shift;
        self.set(word, (self.get(word) & !mask) | ((value << shift) & mask));
    }

    fn channels(self) -> usize {
        1 + self.field(1, 29, 1) as usize
    }
}

impl Xma {
    fn context(&self, index: u32) -> Option<Context> {
        ((index as usize) < self.voices.len() && self.array != 0)
            .then(|| Context((self.array + index as usize * 64) as *mut u32))
    }

    fn enable(&self, index: u32) {
        let Some(context) = self.context(index) else {
            return;
        };
        let mut slot = self.voices[index as usize].lock().unwrap_or_else(|e| e.into_inner());
        context.set_field(2, 26, 6, 0);
        dump_input(index, context);
        *slot = Some(Voice {
            decoder: None,
            pcm: VecDeque::new(),
            skip: context.field(1, 24, 3) as usize * SUBFRAME_SAMPLES,
            ended: false,
        });
        drop(slot);
        wake_decoder();
    }

    fn disable(&self, index: u32) {
        let Some(_) = self.context(index) else {
            return;
        };
        let _ = self.voices[index as usize]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }

    fn advance_read(&self, index: u32, blocks: u32) {
        let Some(context) = self.context(index) else {
            return;
        };
        let slot = self.voices[index as usize].lock().unwrap_or_else(|e| e.into_inner());
        let size = context.field(0, 22, 5).max(1);
        let read = (context.field(9, 0, 5) + blocks % size) % size;
        context.set_field(9, 0, 5, read);
        if blocks > 0 {
            context.set_field(1, 31, 1, 1);
        }
        drop(slot);
        wake_decoder();
    }

    fn pump(&self) {
        for index in 0..self.voices.len() as u32 {
            if let (Some(context), Ok(mut slot)) = (
                self.context(index),
                self.voices[index as usize].lock(),
            ) {
                if let Some(voice) = slot.as_mut() {
                    pump_one(context, voice);
                }
            }
        }
    }
}

fn apu_bytes(apu: u32, len: usize) -> Option<&'static mut [u8]> {
    if len == 0 {
        return Some(&mut []);
    }
    let p = unsafe { crate::hle::acphal::ApuMapApuAddress(apu) };
    if p.is_null() {
        return None;
    }
    // SAFETY: mapped APU region; title and decoder share it.
    Some(unsafe { std::slice::from_raw_parts_mut(p as *mut u8, len) })
}

fn pump_one(context: Context, voice: &mut Voice) {
    let size = context.field(0, 22, 5);
    if size == 0 {
        return;
    }
    while context.field(1, 31, 1) == 1 {
        for _ in 0..8 {
            if voice.pcm.len() >= SAMPLES_PER_BLOCK || voice.ended {
                break;
            }
            if !decode_packets(context, voice) {
                break;
            }
        }
        if voice.pcm.is_empty() || (voice.pcm.len() < SAMPLES_PER_BLOCK && !voice.ended) {
            return;
        }
        let write = context.field(0, 27, 5) % size;
        let address = context.get(7).wrapping_add(write * BLOCK_BYTES as u32);
        let Some(out) = apu_bytes(address, BLOCK_BYTES) else {
            return;
        };
        for pair in out.chunks_exact_mut(2) {
            let sample = voice.pcm.pop_front().unwrap_or(0);
            pair.copy_from_slice(&sample.to_le_bytes());
        }
        let write = (write + 1) % size;
        context.set_field(0, 27, 5, write);
        if write == context.field(9, 0, 5) % size {
            context.set_field(1, 31, 1, 0);
        }
    }
}

/// Decode one XMA frame into `voice.pcm`; false when nothing further is ready.
fn decode_packets(context: Context, voice: &mut Voice) -> bool {
    let valid = context.field(0, 20, 2);
    if voice.ended {
        if valid == 0 {
            return false;
        }
        voice.ended = false;
        if context.field(2, 26, 5) == INPUT_INVALID {
            context.set_field(2, 26, 6, 0);
        }
    }
    let channels = context.channels();
    let mut current = context.field(4, 31, 1);
    if valid & (1 << current) == 0 {
        if valid & (1 << (1 - current)) == 0 {
            if let Some(decoder) = &mut voice.decoder {
                voice.pcm.extend(decoder.flush_tail_i16());
            }
            voice.ended = true;
            context.set_field(2, 26, 5, INPUT_INVALID);
            context.set_field(2, 31, 1, 1);
            return !voice.pcm.is_empty();
        }
        // Dual-input ping-pong: reset the bit cursor only. Keep decoder overlap so
        // consecutive buffers of one voice seam cleanly (CGBJ does not flush here).
        current = 1 - current;
        context.set_field(4, 31, 1, current);
        context.set_field(2, 0, 26, 0);
    }
    let (address, packets) = match current {
        0 => (context.get(5), context.field(0, 0, 12)),
        _ => (context.get(6), context.field(1, 0, 12)),
    };
    if packets == 0 {
        context.set_field(0, 20 + current, 1, 0);
        return true;
    }
    let Some(data) = apu_bytes(address & !0x7ff, packets as usize * xma2::PACKET_SIZE) else {
        context.set_field(0, 20 + current, 1, 0);
        return true;
    };
    let rate_idx = context.field(1, 27, 2) as usize;
    const RATES: [u32; 4] = [24_000, 32_000, 44_100, 48_000];
    let rate = RATES[rate_idx.min(3)];
    let decoder = match voice.decoder.as_mut() {
        Some(d) => d,
        None => {
            let cfg = StreamConfig {
                version: xma2::Version::Xma2,
                sample_rate: rate,
                channels: channels as u16,
                stream_channels: vec![channels.min(2) as u8],
            };
            match Decoder::new(cfg) {
                Ok(d) => {
                    voice.decoder = Some(d);
                    voice.decoder.as_mut().unwrap()
                }
                Err(e) => {
                    crate::xlog_warn!("SHAPE XMA: decoder init failed: {e}");
                    context.set_field(0, 20 + current, 1, 0);
                    return false;
                }
            }
        }
    };
    // Mid-packet seek: word-2 bit cursor may land past the packet header inside a frame chain.
    let at_bits = xma2::frames::seek_bit(data, context.field(2, 0, 26) as usize);
    let Some(frame) = xma2::frames::read_frame(data, at_bits) else {
        context.set_field(0, 20 + current, 1, 0);
        return true;
    };
    let mut pcm = decoder
        .decode_frame_exact(&frame.data, frame.bits)
        .unwrap_or_default();
    let mut next = frame.next.unwrap_or(0);
    let loops = context.field(0, 12, 8);
    let loop_end = context.field(4, 0, 26) as usize;
    let ends_loop = loop_end != 0
        && (frame.start == loop_end
            || (frame.start < loop_end && frame.next.is_none_or(|n| loop_end < n)));
    let looping = loops > 0 && ends_loop;
    if looping {
        let keep = (context.field(1, 12, 2) as usize + 1) * SUBFRAME_SAMPLES * channels;
        if pcm.len() > keep {
            pcm.truncate(keep);
        }
        // Seek to loop start bit (context word 3); keep IMDCT overlap across the loop edge.
        next = context.field(3, 0, 26) as usize;
        if loops != 255 {
            context.set_field(0, 12, 8, loops - 1);
        }
    }
    let skip = (voice.skip * channels).min(pcm.len());
    voice.skip -= skip / channels;
    voice.pcm.extend(&pcm[skip..]);
    if looping {
        voice.skip = context.field(1, 17, 3) as usize * SUBFRAME_SAMPLES;
    }
    if next == 0 {
        context.set_field(0, 20 + current, 1, 0);
    }
    context.set_field(2, 0, 26, next as u32);
    true
}

/// `XCL_XMA_DUMP=<dir>`: save context words + input 0 for the first 32 enables.
fn dump_input(index: u32, context: Context) {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let Some(dir) = std::env::var_os("XCL_XMA_DUMP").or_else(|| std::env::var_os("VICTORY_XMA_DUMP")) else {
        return;
    };
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= 32 {
        return;
    }
    let dir = std::path::PathBuf::from(dir);
    let words: Vec<u8> = (0..CONTEXT_WORDS)
        .flat_map(|w| context.get(w).to_le_bytes())
        .collect();
    let input = apu_bytes(
        context.get(5) & !0x7ff,
        context.field(0, 0, 12) as usize * xma2::PACKET_SIZE,
    );
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{n:02}-{index}.ctx")), words);
    let _ = std::fs::write(dir.join(format!("{n:02}-{index}.xma")), input.as_deref().unwrap_or(&[]));
    crate::xlog_info!("XCL_XMA_DUMP: wrote {n:02}-{index}.ctx/.xma under {}", dir.display());
}

fn wake_decoder() {
    static DECODER: OnceLock<std::thread::Thread> = OnceLock::new();
    DECODER
        .get_or_init(|| {
            std::thread::Builder::new()
                .name("xcl XMA decoder".into())
                .spawn(|| loop {
                    xma_read().pump();
                    std::thread::park_timeout(std::time::Duration::from_millis(2));
                })
                .map_or_else(|_| std::thread::current(), |h| h.thread().clone())
        })
        .unpark();
}

/// Runs an XMA command from AcpHal `SubmitCommand`; false if unknown.
pub fn command(command: u32, word: impl Fn(usize) -> Option<u32>) -> bool {
    let xma = xma_read();
    match command {
        ENABLE_XMA => xma.enable(word(0).unwrap_or(u32::MAX)),
        DISABLE_XMA => xma.disable(word(0).unwrap_or(u32::MAX)),
        ENABLE_XMAS | DISABLE_XMAS => {
            for i in 0..xma.voices.len() as u32 {
                if word(i as usize / 32).unwrap_or(0) & (1 << (i % 32)) != 0 {
                    match command {
                        ENABLE_XMAS => xma.enable(i),
                        _ => xma.disable(i),
                    }
                }
            }
        }
        ADVANCE_XMA_READ => xma.advance_read(word(0).unwrap_or(u32::MAX), word(1).unwrap_or(0)),
        _ => return false,
    }
    true
}

/// True when this AcpHal command id is an XMA control word.
pub fn is_xma_command(command: u32) -> bool {
    matches!(
        command,
        ENABLE_XMA | ENABLE_XMAS | DISABLE_XMA | DISABLE_XMAS | ADVANCE_XMA_READ
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xma_command_ids_match_console() {
        assert!(is_xma_command(ENABLE_XMA));
        assert!(is_xma_command(ADVANCE_XMA_READ));
        assert!(!is_xma_command(1));
        assert!(!is_xma_command(2));
    }

    #[test]
    fn allocate_and_enable_without_crash() {
        on_shape_released();
        let mut words = [0u32; CONTEXT_WORDS];
        // writable, mono 48 kHz, one packet input, 4-block ring
        words[0] = 1 | (4 << 22) | (1 << 20);
        words[1] = (3 << 27) | (1 << 31);
        words[7] = 0;
        on_shape_allocated(
            &[0, 0, 0, 0, words.as_mut_ptr() as usize, 0],
            &[0, 0, 0, 0, 1, 0],
        );
        assert!(command(ENABLE_XMA, |_| Some(0)));
        assert!(command(ADVANCE_XMA_READ, |i| Some(if i == 0 { 0 } else { 1 })));
        assert!(command(DISABLE_XMA, |_| Some(0)));
        on_shape_released();
    }
}
