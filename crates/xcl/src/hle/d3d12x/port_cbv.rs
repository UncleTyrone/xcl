//! Renderer port 5 (`XCL_PORT_CBV`): constant buffers in ported title memory are exposed as raw
//! buffer views (16-byte aligned) in the slot's twin descriptor; shaders compiled with
//! [`crate::gpu::recomp::constants_raw`] read through that twin.

use std::sync::atomic::{AtomicU64, Ordering};

static STATS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

pub fn on() -> bool {
    super::port::cbv()
}

/// Shaders use raw buffer loads when this is true (paired with [`on`]).
pub fn constants_raw() -> bool {
    on()
}

pub fn in_place_on() -> bool {
    on() && std::env::var_os("XCL_PORT_CBV_PLACE").is_none_or(|v| v != "0")
}

pub fn raw_ok(gpu_offset: u64) -> bool {
    gpu_offset.is_multiple_of(16)
}

pub fn in_place(bytes: u32) {
    STATS[0].fetch_add(1, Ordering::Relaxed);
    STATS[1].fetch_add(u64::from(bytes), Ordering::Relaxed);
}

pub fn copied(misaligned: bool) {
    STATS[if misaligned { 3 } else { 2 }].fetch_add(1, Ordering::Relaxed);
}

pub fn twinned() {
    STATS[4].fetch_add(1, Ordering::Relaxed);
}

/// Twin descriptor words: raw byte view at `address` for `size` bytes.
pub fn raw_words(address: u64, size: u32) -> Option<[u32; 4]> {
    if !on() || !raw_ok(address) || size == 0 {
        return None;
    }
    let bytes = size.max(4).next_multiple_of(4);
    Some(crate::gpu::desc::buffer_words(address, 0, bytes, 32))
}

pub fn tick_log() {
    if !on() {
        return;
    }
    let s: Vec<u64> = STATS.iter().map(|c| c.swap(0, Ordering::Relaxed)).collect();
    if s.iter().any(|&x| x != 0) {
        crate::xlog_info!(
            "D3D12.X: port 5 (XCL_PORT_CBV=1): {:.0} in-place ({:.1} KiB), {:.0} copied, {:.0} misaligned, {:.0} raw twins",
            s[0],
            s[1] as f64 / 1024.0,
            s[2],
            s[3],
            s[4],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_alignment() {
        assert!(raw_ok(0x34e1_0010));
        assert!(!raw_ok(0x34e1_0008));
    }
}