//! Buffer copies at their place in the list (`XCL_COPIES_IN_PLACE`, default on).
//!
//! Tracks how often title buffer copies are kept in-list vs deferred to
//! submission-time CPU copies when the latch is off or the span is too large.

use std::sync::atomic::{AtomicU64, Ordering};

use super::super::port;

/// Most bytes copied out of memory shaders do not write (streaming threshold).
pub(crate) const MOST: usize = 8 << 20;

pub(crate) fn on() -> bool {
    port::copies_in_place()
}

static STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
const S_COPIES: usize = 0;
const S_BYTES: usize = 1;
const S_FROM_HOST: usize = 2;
const S_TEXTURE: usize = 3;

/// A buffer copy recorded at its place (`from_host`: source is a host/shadow copy).
pub(crate) fn note_copy(len: usize, from_host: bool) {
    if !on() {
        return;
    }
    STATS[S_COPIES].fetch_add(1, Ordering::Relaxed);
    STATS[S_BYTES].fetch_add(len as u64, Ordering::Relaxed);
    if from_host {
        STATS[S_FROM_HOST].fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn note_texture_from_host() {
    if on() {
        STATS[S_TEXTURE].fetch_add(1, Ordering::Relaxed);
    }
}

/// Whether a span this large should stay on the CPU path even when in-place is on.
pub(crate) fn too_large(len: usize, shaders_write_src: bool) -> bool {
    !shaders_write_src && len > MOST
}

pub(crate) fn line(frames: f64) {
    let f = frames.max(1.0);
    let copies = STATS[S_COPIES].swap(0, Ordering::Relaxed) as f64 / f;
    let bytes = STATS[S_BYTES].swap(0, Ordering::Relaxed) as f64 / f;
    let host = STATS[S_FROM_HOST].swap(0, Ordering::Relaxed) as f64 / f;
    let tex = STATS[S_TEXTURE].swap(0, Ordering::Relaxed) as f64 / f;
    if copies > 0.0 || tex > 0.0 {
        crate::xlog_info!(
            "D3D12.X: XCL_COPIES_IN_PLACE per frame: {copies:.1} copies ({:.1} KB, {host:.1} from host copy), {tex:.1} texture from host",
            bytes / 1024.0
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_threshold() {
        assert!(too_large(MOST + 1, false));
        assert!(!too_large(MOST + 1, true));
        assert!(!too_large(1024, false));
    }
}
