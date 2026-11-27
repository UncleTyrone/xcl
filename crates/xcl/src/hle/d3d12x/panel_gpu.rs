//! GPU diagnostics overlay for the host panel (`XCL_PANEL=1` + `XCL_PANEL_GPU=1`).
//!
//! Copies a CPU-rasterised status box onto the swap-chain back buffer before present.
//! CGBJ keeps the painted box in a **card-local DEFAULT** buffer and rotates uploads; this GPL-2
//! path mirrors that (upload ring → DEFAULT resident buffer → blit to back buffer).
//!
//! **Atlas gap (honest PARTIAL):** CGBJ-style per-glyph texture tiles on the title heap are **not**
//! replicated. Glyphs are CPU-rasterised into the box each refresh; only the completed box lives
//! on the card. Multi-line + height scale + common swap formats are supported.

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

const GLYPH: (u32, u32) = (5, 7);
const CELL: (u32, u32) = (6, 9);
const COLUMNS: u32 = 40;
const LINES: u32 = 4;
const RIM: u32 = 3;
const UPLOADS: usize = 4;

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

static OVERLAY_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub(crate) fn set_overlay_text(line: impl Into<String>) {
    let s = line.into();
    let lines: Vec<String> = s
        .lines()
        .map(|l| l.chars().take(COLUMNS as usize).collect())
        .filter(|l: &String| !l.is_empty())
        .take(LINES as usize)
        .collect();
    if let Ok(mut g) = OVERLAY_LINES.lock() {
        *g = if lines.is_empty() {
            vec![s.chars().take(COLUMNS as usize).collect()]
        } else {
            lines
        };
    }
}

/// Seven rows of five dots (left bit highest), GPL-2 font matching the host panel alphabet.
#[rustfmt::skip]
fn glyph_rows(c: u8) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        b'0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        b'1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        b'3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        b'4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        b'5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        b'6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        b'7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b10000],
        b'8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        b'9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        b'A' => [0b00100, 0b01010, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001],
        b'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        b'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        b'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        b'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        b'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        b'G' => [0b01111, 0b10000, 0b10000, 0b10011, 0b10001, 0b10001, 0b01111],
        b'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        b'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        b'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        b'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        b'N' => [0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001],
        b'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        b'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        b'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        b'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        b'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        b'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        b'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        b'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        b'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        b'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        b'.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        b'/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        b'-' => [0, 0, 0, 0b11111, 0, 0, 0],
        b'=' => [0, 0b11111, 0, 0b11111, 0, 0, 0],
        b' ' => [0; 7],
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}

fn scale_for(height: u32) -> u32 {
    (height / 540).max(1).min(4)
}

fn box_size(scale: u32, line_count: u32) -> (u32, u32) {
    let lines = line_count.max(1).min(LINES);
    (
        (COLUMNS * CELL.0 + 2 * RIM) * scale,
        (lines * CELL.1 + 2 * RIM) * scale,
    )
}

fn colours(format: DXGI_FORMAT) -> Option<[&'static [u8]; 2]> {
    Some(match format {
        DXGI_FORMAT_R8G8B8A8_UNORM
        | DXGI_FORMAT_B8G8R8A8_UNORM
        | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => [&[0, 0, 0, 0xFF], &[0xFF; 4]],
        DXGI_FORMAT_R10G10B10A2_UNORM => [&[0, 0, 0, 0xC0], &[0xFF; 4]],
        DXGI_FORMAT_R16G16B16A16_FLOAT => [
            &[0, 0, 0, 0, 0, 0, 0, 0x3C],
            &[0, 0x3C, 0, 0x3C, 0, 0x3C, 0, 0x3C],
        ],
        _ => return None,
    })
}

fn lit(lines: &[String], x: u32, y: u32) -> bool {
    let (Some(x), Some(y)) = (x.checked_sub(RIM), y.checked_sub(RIM)) else {
        return false;
    };
    if x % CELL.0 >= GLYPH.0 || y % CELL.1 >= GLYPH.1 || x / CELL.0 >= COLUMNS {
        return false;
    }
    let Some(&c) = lines
        .get((y / CELL.1) as usize)
        .and_then(|l| l.as_bytes().get((x / CELL.0) as usize))
    else {
        return false;
    };
    (glyph_rows(c)[(y % CELL.1) as usize] >> (GLYPH.0 - 1 - x % CELL.0)) & 1 != 0
}

fn painted(lines: &[String], scale: u32, colours: [&[u8]; 2], pitch: usize) -> Vec<u8> {
    let (width, height) = box_size(scale, lines.len() as u32);
    let bytes = colours[0].len();
    let mut out = vec![0u8; pitch * height as usize];
    for y in 0..height / scale {
        let top = pitch * (y * scale) as usize;
        for x in 0..width / scale {
            let colour = colours[usize::from(lit(lines, x, y))];
            for i in 0..scale {
                let at = top + (x * scale + i) as usize * bytes;
                if at + bytes <= out.len() {
                    out[at..at + bytes].copy_from_slice(colour);
                }
            }
        }
        for i in 1..scale as usize {
            let src = top..top + pitch;
            let dst_start = top + i * pitch;
            if dst_start + pitch <= out.len() {
                out.copy_within(src, dst_start);
            }
        }
    }
    out
}

fn simple_hash(lines: &[String], scale: u32, fmt: i32) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    h ^= u64::from(scale);
    h = h.wrapping_mul(0x100000001b3);
    h ^= u64::from(fmt as u32);
    h = h.wrapping_mul(0x100000001b3);
    for line in lines {
        for b in line.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0x0A;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

struct GpuOverlay {
    uploads: Vec<ID3D12Resource>,
    /// Card-local DEFAULT buffer holding the last painted box (CGBJ panel role).
    card: ID3D12Resource,
    turn: usize,
    w: u32,
    h: u32,
    pitch: u32,
    format: DXGI_FORMAT,
    scale: u32,
    /// `card` already holds a copy and sits in COPY_SOURCE.
    uploaded: bool,
    fresh: bool,
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

unsafe fn ensure_overlay(
    dev: &ID3D12Device,
    lines: &[String],
    fmt: DXGI_FORMAT,
    scale: u32,
) -> Option<()> {
    let hash = simple_hash(lines, scale, fmt.0);
    let mut g = OVERLAY.lock().ok()?;
    if g.as_ref().is_some_and(|(h, o)| *h == hash && o.format == fmt && o.scale == scale) {
        return Some(());
    }
    let colours = colours(fmt)?;
    let (w, h) = box_size(scale, lines.len() as u32);
    let pitch = (w * colours[0].len() as u32).div_ceil(256) * 256;
    let pixels = painted(lines, scale, colours, pitch as usize);
    let bytes = pixels.len() as u64;

    let mut uploads = Vec::with_capacity(UPLOADS);
    for _ in 0..UPLOADS {
        let mut upload: Option<ID3D12Resource> = None;
        dev.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_UPLOAD,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: bytes,
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
        uploads.push(upload?);
    }

    let mut card: Option<ID3D12Resource> = None;
    dev.CreateCommittedResource(
        &D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        },
        D3D12_HEAP_FLAG_NONE,
        &D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_NONE,
            ..Default::default()
        },
        D3D12_RESOURCE_STATE_COMMON,
        None,
        &mut card,
    )
    .ok()?;
    let card = card?;

    let mut mapped = std::ptr::null_mut();
    uploads[0].Map(0, None, Some(&mut mapped)).ok()?;
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), mapped.cast(), pixels.len());
    uploads[0].Unmap(0, None);

    *g = Some((
        hash,
        GpuOverlay {
            uploads,
            card,
            turn: 0,
            w,
            h,
            pitch,
            format: fmt,
            scale,
            uploaded: false,
            fresh: true,
        },
    ));
    Some(())
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
    if colours(fmt).is_none() {
        if !WARNED_FMT.swap(true, Ordering::Relaxed) {
            crate::xlog_info!(
                "D3D12.X: XCL_PANEL_GPU skipped — swap format {:?} unsupported (need 8-bit RGBA / 10:10:10:2 / half RGBA; log/title panel still on). Card-local per-glyph atlas not implemented.",
                fmt.0
            );
        }
        return;
    }
    let lines = OVERLAY_LINES.lock().ok().map(|g| g.clone()).unwrap_or_default();
    if lines.is_empty() || lines.iter().all(|l| l.is_empty()) {
        return;
    }
    let scale = scale_for(back_h);
    if ensure_overlay(dev, &lines, fmt, scale).is_none() {
        return;
    }
    let mut guard = match OVERLAY.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some((_, ov)) = guard.as_mut() else {
        return;
    };
    let copy_w = ov.w.min(back_w);
    let copy_h = ov.h.min(back_h);
    if copy_w == 0 || copy_h == 0 {
        return;
    }

    if std::mem::take(&mut ov.fresh) {
        if ov.uploaded {
            list.ResourceBarrier(&[barrier(
                &ov.card,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )]);
        }
        list.CopyResource(&ov.card, &ov.uploads[ov.turn]);
        list.ResourceBarrier(&[barrier(
            &ov.card,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )]);
        ov.uploaded = true;
    }

    list.ResourceBarrier(&[barrier(back, before, D3D12_RESOURCE_STATE_COPY_DEST)]);
    let src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(ov.card.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: if matches!(
                        fmt,
                        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
                    ) {
                        DXGI_FORMAT_B8G8R8A8_UNORM
                    } else if fmt == DXGI_FORMAT_R10G10B10A2_UNORM {
                        DXGI_FORMAT_R10G10B10A2_UNORM
                    } else if fmt == DXGI_FORMAT_R16G16B16A16_FLOAT {
                        DXGI_FORMAT_R16G16B16A16_FLOAT
                    } else {
                        DXGI_FORMAT_R8G8B8A8_UNORM
                    },
                    Width: copy_w,
                    Height: copy_h,
                    Depth: 1,
                    RowPitch: ov.pitch,
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
