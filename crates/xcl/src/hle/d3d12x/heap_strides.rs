//! View-stride / kind table for table-indexed typed buffer loads (CGBJ P1).
//!
//! Opt in with `XCL_HEAP_STRIDES=1` (off by default). Populates a dword per shared-heap
//! view index (byte stride in low 30 bits, [`VIEW_INTEGER`] / [`VIEW_SIGNED`] above).

use std::sync::{Mutex, OnceLock};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;

/// Root SRV slot for the stride table in translated shaders (`t252`, space 1 in CGBJ).
pub const SLOT_TABLE_STRIDES: u8 = 252;
/// Low bits of a table dword: byte stride of the buffer view.
pub const VIEW_STRIDE_BITS: u32 = (1 << 30) - 1;
/// Typed integer UINT view (d16 passes raw bits).
pub const VIEW_INTEGER: u32 = 1 << 31;
/// Typed integer SINT view.
pub const VIEW_SIGNED: u32 = 1 << 30;
/// Dwords in the table (clamp heap indices; matches shader heap limit).
pub const TABLE_WORDS: u32 = crate::gpu::dxil::lower_image::VIEW_HEAP_MAX_SRV;

struct Table {
    cpu: Vec<u32>,
    gpu: u64,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

/// `XCL_HEAP_STRIDES=1` enables stride table maintenance and DXIL integer d16 paths.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("XCL_HEAP_STRIDES")
            .is_ok_and(|v| matches!(v.trim(), "1" | "on" | "ON" | "true" | "TRUE"))
    })
}

fn init_table(g: &mut Option<Table>) {
    let words = TABLE_WORDS as usize;
    let Some(u) = crate::gpu::uma::get() else { return };
    let bytes = words * 4;
    let Some(p) = u.alloc(bytes.max(256), 256) else { return };
    unsafe { std::ptr::write_bytes(p as *mut u8, 0, bytes) };
    let Some(gpu) = super::real::uma_va_of(p as usize) else { return };
    *g = Some(Table { cpu: vec![0u32; words], gpu });
}

/// Bind the stride table on an open list (`bind_abi` root index from [`crate::gpu::recomp::pso::abi_heap_strides_root_index`]).
pub unsafe fn bind_root(list: &windows::Win32::Graphics::Direct3D12::ID3D12GraphicsCommandList, gfx: bool, root_index: u32) {
    if let Some(gpu) = gpu_address() {
        if gfx {
            list.SetGraphicsRootShaderResourceView(root_index, gpu);
        } else {
            list.SetComputeRootShaderResourceView(root_index, gpu);
        }
    }
}

/// GPU VA of the stride table for root binding; `None` when off or not allocated.
pub fn gpu_address() -> Option<u64> {
    if !on() {
        return None;
    }
    let g = TABLE.lock().unwrap();
    g.as_ref().map(|t| t.gpu)
}

/// Update dword `index` (shared-heap SRV index) from a published view of `res` (`0` clears stride).
pub fn note_view(index: u32, res: usize) {
    if !on() {
        return;
    }
    if res == 0 {
        write(index, 0);
        return;
    }
    let Some(word) = stride_word_for_resource(res) else { return };
    write(index, word);
}

pub fn write(index: u32, word: u32) {
    if !on() {
        return;
    }
    let mut g = TABLE.lock().unwrap();
    if g.is_none() {
        init_table(&mut g);
    }
    let Some(t) = g.as_mut() else { return };
    if index >= t.cpu.len() as u32 {
        return;
    }
    t.cpu[index as usize] = word;
    let Some(u) = crate::gpu::uma::get() else { return };
    let p = (t.gpu as usize).wrapping_sub(u.base());
    if p + 4 <= u.size() {
        unsafe {
            (p as *mut u32).add(index as usize).write_volatile(word);
        }
    }
}

/// DXGI format → stride dword with integer kind bits (typed non-raw buffer views).
pub(crate) fn stride_word_for_format(format: u32, element_bytes: u32) -> u32 {
    let stride = element_bytes.min(VIEW_STRIDE_BITS);
    match integer_format(format) {
        Some(false) => stride | VIEW_INTEGER,
        Some(true) => stride | VIEW_INTEGER | VIEW_SIGNED,
        None => stride,
    }
}

fn stride_word_for_resource(res: usize) -> Option<u32> {
    let p = res as *mut std::ffi::c_void;
    let r = unsafe { ID3D12Resource::from_raw_borrowed(&p)? };
    let desc = unsafe { r.GetDesc() };
    if desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER {
        return None;
    }
    let element = buffer_element_bytes(desc.Format.0 as u32) as u32;
    Some(stride_word_for_format(desc.Format.0 as u32, element))
}

fn buffer_element_bytes(format: u32) -> u64 {
    match format {
        0..=2 => 16,
        3..=8 => 4,
        12 | 14 => 8,
        17 | 18 => 4,
        25..=30 | 32 | 36 | 38 | 42 | 43 | 50 | 52 | 57 | 59 | 62 | 64 => 2,
        _ => 4,
    }
}

fn integer_format(format: u32) -> Option<bool> {
    match format {
        3 | 7 | 12 | 17 | 25 | 30 | 36 | 42 | 50 | 57 | 62 => Some(false),
        4 | 8 | 14 | 18 | 32 | 38 | 43 | 52 | 59 | 64 => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_formats_tag_view_kind_bits() {
        assert_eq!(stride_word_for_format(12, 8), 8 | VIEW_INTEGER);
        assert_eq!(stride_word_for_format(59, 2), 2 | VIEW_INTEGER | VIEW_SIGNED);
        assert_eq!(stride_word_for_format(2, 16), 16);
    }
}