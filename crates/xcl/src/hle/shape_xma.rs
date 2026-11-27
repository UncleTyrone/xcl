//! ACPHAL SHAPE ↔ XMA path (CGBJ `audio.rs` XMA contexts).
//!
//! XCL's `AcpHalAllocateShapeContexts` keeps a **5-kind** layout (observed ABI),
//! not CGBJ's packed 6-kind `{counts, arrays, apu}`. XMA contexts live at
//! **kind index 4** (same index as CGBJ's XMA slot among its six). Commands
//! `ENABLE_XMA` / `DISABLE_XMA` / `ENABLE_XMAS` / `DISABLE_XMAS` /
//! `ADVANCE_XMA_READ` on the AcpHal channel drive a decoder thread that fills
//! each enabled context's 256-byte PCM output ring from its input packets via
//! [`crate::hle::xma2::Decoder`].
//!
//! Still **ABSENT / thinner than CGBJ**:
//! - 6th SHAPE kind (PCM contexts) and per-kind context sizes
//! - bit-accurate mid-packet frame seek / loop / overlap-buffer handoff
//! - subframe skip fields beyond initial packet decode

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
/// Kind index in XCL's 5-slot SHAPE table that holds XMA contexts (matches CGBJ index 4).
pub const XMA_KIND: usize = 4;

struct Voice {
    decoder: Option<Decoder>,
    pcm: VecDeque<i16>,
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

/// Bind the XMA decoder to SHAPE kind-4 contexts after allocation.
pub fn on_shape_allocated(arrays: &[usize; 5], counts: &[u32; 5]) {
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
        if voice.pcm.len() < SAMPLES_PER_BLOCK && !voice.ended {
            decode_packets(context, voice);
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

fn decode_packets(context: Context, voice: &mut Voice) {
    let valid = context.field(0, 20, 2);
    if voice.ended {
        if valid == 0 {
            return;
        }
        voice.ended = false;
    }
    let mut current = context.field(4, 31, 1);
    if valid & (1 << current) == 0 {
        if valid & (1 << (1 - current)) == 0 {
            if let Some(decoder) = &mut voice.decoder {
                if let Ok(tail) = decoder.decode_buffer_i16(&[], true) {
                    voice.pcm.extend(tail);
                }
            }
            voice.ended = true;
            context.set_field(2, 26, 5, 4);
            context.set_field(2, 31, 1, 1);
            return;
        }
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
        return;
    }
    let Some(data) = apu_bytes(address & !0x7ff, packets as usize * xma2::PACKET_SIZE) else {
        context.set_field(0, 20 + current, 1, 0);
        return;
    };
    let channels = context.channels() as u16;
    const RATES: [u32; 4] = [24_000, 32_000, 44_100, 48_000];
    let rate = RATES[context.field(1, 27, 2) as usize];
    let decoder = match voice.decoder.as_mut() {
        Some(d) => d,
        None => {
            let cfg = StreamConfig {
                version: xma2::Version::Xma2,
                sample_rate: rate,
                channels,
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
                    return;
                }
            }
        }
    };
    match decoder.decode_buffer_i16(data, false) {
        Ok(pcm) => voice.pcm.extend(pcm),
        Err(e) => crate::xlog_warn!("SHAPE XMA: decode failed: {e}"),
    }
    // Consume this input buffer; titles refill and re-enable as needed.
    context.set_field(0, 20 + current, 1, 0);
    context.set_field(2, 0, 26, packets * xma2::PACKET_SIZE as u32 * 8);
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
        on_shape_allocated(&[0, 0, 0, 0, words.as_mut_ptr() as usize], &[0, 0, 0, 0, 1]);
        assert!(command(ENABLE_XMA, |_| Some(0)));
        assert!(command(ADVANCE_XMA_READ, |i| Some(if i == 0 { 0 } else { 1 })));
        assert!(command(DISABLE_XMA, |_| Some(0)));
        on_shape_released();
    }
}
