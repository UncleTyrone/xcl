//! GPU diagnostics overlay for the host panel (`XCL_PANEL=1` + `XCL_PANEL_GPU=1`).
//!
//! Copies a small CPU-rasterised status box onto the swap-chain back buffer before present.
//! CGBJ draws glyphs on the title picture via a card-local texture; this is a GPL-2 subset
//! (log line + optional window title still run from [`super::panel`]).

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

const CELL_W: u32 = 6;
const GLYPH_W: u32 = 5;
const RIM: u32 = 3;

fn panel_gpu_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("XCL_PANEL_GPU")
            .or_else(|_| std::env::var("VICTORY_PANEL_GPU"))
            .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
    })
}

pub(crate) fn gpu_wanted() -> bool {
    super::panel::visible() && panel_gpu_on()
}

static OVERLAY_TEXT: Mutex<String> = Mutex::new(String::new());

pub(crate) fn set_overlay_text(line: impl Into<String>) {
    if let Ok(mut g) = OVERLAY_TEXT.lock() {
        *g = line.into();
    }
}

#[rustfmt::skip]
fn glyph_rows(c: u8) -> [u8; 7] {
    match c {
        b' ' => [0; 7],
        b'.' => [0, 0, 0, 0, 0, 0b00100, 0],
        b'-' => [0, 0, 0, 0b11111, 0, 0, 0],
        b'/' => [0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0, 0],
        b'0'..=b'9' => {
            const T: [[u8; 7]; 10] = [
                [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
                [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
                [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
                [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
                [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
                [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
                [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
                [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b10000],
                [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
                [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
            ];
            T[(c - b'0') as usize]
        }
        b'A'..=b'Z' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'a'..=b'z' => glyph_rows(c.to_ascii_uppercase()),
        b'=' => [0, 0b11111, 0, 0b11111, 0, 0, 0],
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}

fn paint_rgba8(line: &str) -> (u32, u32, u32, Vec<u8>) {
    let cols = line.len().min(38).max(1) as u32;
    let w = RIM * 2 + cols * CELL_W;
    let h = RIM * 2 + 9;
    let pitch = (w * 4).div_ceil(256) * 256;
    let mut buf = vec![0u8; (pitch * h) as usize];
    let bg = [0u8, 0u8, 0u8, 200];
    let fg = [240, 240, 240, 255];
    for y in 0..h {
        for x in 0..w {
            let i = y as usize * pitch as usize + x as usize * 4;
            buf[i..i + 4].copy_from_slice(&bg);
        }
    }
    for (ci, ch) in line.bytes().take(38).enumerate() {
        let rows = glyph_rows(ch);
        let ox = RIM + ci as u32 * CELL_W;
        let oy = RIM;
        for (ry, row) in rows.iter().enumerate() {
            for gx in 0..GLYPH_W {
                if row & (1 << (GLYPH_W - 1 - gx)) != 0 {
                    let px = ox + gx;
                    let py = oy + ry as u32;
                    if px < w && py < h {
                        let i = py as usize * pitch as usize + px as usize * 4;
                        buf[i..i + 4].copy_from_slice(&fg);
                    }
                }
            }
        }
    }
    (w, h, pitch, buf)
}

fn simple_hash(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

struct GpuOverlay {
    upload: ID3D12Resource,
    w: u32,
    h: u32,
    pitch: u32,
}

static OVERLAY: Mutex<Option<(u64, GpuOverlay)>> = Mutex::new(None);
static WARNED_FMT: AtomicBool = AtomicBool::new(false);

unsafe fn barrier(res: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::transmute_copy(res),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

unsafe fn refresh_upload(dev: &ID3D12Device, line: &str) -> Option<()> {
    let hash = simple_hash(line);
    let mut g = OVERLAY.lock().ok()?;
    if g.as_ref().is_some_and(|(h, _)| *h == hash) {
        return Some(());
    }
    let (w, h, pitch, pixels) = paint_rgba8(line);
    let mut upload: Option<ID3D12Resource> = None;
    dev.CreateCommittedResource(
        &D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            ..Default::default()
        },
        D3D12_HEAP_FLAG_NONE,
        &D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: pixels.len() as u64,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_NONE,
            ..Default::default()
        },
        D3D12_RESOURCE_STATE_GENERIC_READ,
        None,
        &mut upload,
    )
    .ok()?;
    let upload = upload?;
    let mut mapped = std::ptr::null_mut();
    upload.Map(0, None, Some(&mut mapped)).ok()?;
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), mapped.cast(), pixels.len());
    upload.Unmap(0, None);
    *g = Some((
        hash,
        GpuOverlay {
            upload,
            w,
            h,
            pitch,
        },
    ));
    Some(())
}

fn overlay_format_ok(fmt: DXGI_FORMAT) -> bool {
    matches!(
        fmt,
        DXGI_FORMAT_R8G8B8A8_UNORM
            | DXGI_FORMAT_B8G8R8A8_UNORM
            | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
            | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
    )
}

/// Copy the status box onto `back` (must already be in `before`; restored to `before` on return).
pub(crate) unsafe fn draw(
    dev: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    back: &ID3D12Resource,
    fmt: DXGI_FORMAT,
    back_w: u32,
    back_h: u32,
    before: D3D12_RESOURCE_STATES,
) {
    if !gpu_wanted() {
        return;
    }
    if !overlay_format_ok(fmt) {
        if !WARNED_FMT.swap(true, Ordering::Relaxed) {
            crate::xlog_info!(
                "D3D12.X: XCL_PANEL_GPU skipped — swap format {:?} needs 8-bit RGBA (log/title panel still on)",
                fmt.0
            );
        }
        return;
    }
    let line = OVERLAY_TEXT.lock().ok().map(|g| g.clone()).unwrap_or_default();
    if line.is_empty() {
        return;
    }
    if refresh_upload(dev, &line).is_none() {
        return;
    }
    let ov = OVERLAY.lock().ok().and_then(|g| g.as_ref().map(|(_, o)| (o.w, o.h, o.pitch, o.upload.clone())));
    let Some((ow, oh, pitch, upload)) = ov else {
        return;
    };
    let copy_w = ow.min(back_w);
    let copy_h = oh.min(back_h);
    if copy_w == 0 || copy_h == 0 {
        return;
    }

    list.ResourceBarrier(&[barrier(back, before, D3D12_RESOURCE_STATE_COPY_DEST)]);
    let src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload)),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    Width: copy_w,
                    Height: copy_h,
                    Depth: 1,
                    RowPitch: pitch,
                },
            },
        },
    };
    let dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(back.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    let part = D3D12_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: copy_w,
        bottom: copy_h,
        back: 1,
    };
    list.CopyTextureRegion(&dst, 0, 0, 0, &src, Some(&part));
    let _ = ManuallyDrop::into_inner(src.pResource);
    let _ = ManuallyDrop::into_inner(dst.pResource);
    list.ResourceBarrier(&[barrier(back, D3D12_RESOURCE_STATE_COPY_DEST, before)]);
}