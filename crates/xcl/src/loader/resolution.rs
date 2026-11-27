//! Title output size and render scale (`XCL_OUTPUT`, `XCL_RENDER_SCALE`, `XCL_FFX_SCALE`,
//! `XCL_UPSCALER`; each off unless set). Byte-checked patches for game_scarlett_master.exe 1.0.2.1
//! (reimplements victory-loader `resolution`).

use super::frame_rate::Patched;

/// Sizes the title has a display mode for (its own list, +0x615c0c0).
pub const MODES: [(u32, u32); 5] = [(1280, 720), (1664, 936), (1920, 1080), (2560, 1440), (3840, 2160)];

/// Console-reported display size (`XCL_DISPLAY_MODE=WxH`, else 3840×2160).
pub fn display_mode_size() -> (u32, u32) {
    static SIZE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        let asked = std::env::var("XCL_DISPLAY_MODE").ok().and_then(|v| {
            let (w, h) = v.trim().split_once(['x', 'X'])?;
            Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
        });
        let size = match asked {
            Some((w, h)) if (640..=7680).contains(&w) && (360..=4320).contains(&h) => (w, h),
            _ => (3840, 2160),
        };
        if std::env::var_os("XCL_DISPLAY_MODE").is_some() {
            crate::xlog_info!("loader: XCL_DISPLAY_MODE → {}×{}", size.0, size.1);
        }
        size
    })
}

pub fn asked_output() -> Option<(u32, u32)> {
    let spec = std::env::var("XCL_OUTPUT").ok()?;
    let size = parse_output(&spec);
    if size.is_none() {
        crate::xlog_warn!("loader: XCL_OUTPUT={spec}: expected 1280x720, 1664x936, 1920x1080, 2560x1440, or 3840x2160");
    }
    size
}

pub fn parse_output(spec: &str) -> Option<(u32, u32)> {
    let (w, h) = spec.trim().split_once(['x', 'X'])?;
    let size = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    MODES.contains(&size).then_some(size)
}

pub fn asked_render_scale() -> Option<f32> {
    let spec = std::env::var("XCL_RENDER_SCALE").ok()?;
    let scale = parse_render_scale(&spec);
    if scale.is_none() {
        crate::xlog_warn!("loader: XCL_RENDER_SCALE={spec}: expected 0.5 to 1.0");
    }
    scale
}

pub fn parse_render_scale(spec: &str) -> Option<f32> {
    spec.trim().parse::<f32>().ok().filter(|s| (0.5..=1.0).contains(s))
}

pub fn asked_ffx_scale() -> Option<f32> {
    let spec = std::env::var("XCL_FFX_SCALE").ok()?;
    let scale = parse_render_scale(&spec);
    if scale.is_none() {
        crate::xlog_warn!("loader: XCL_FFX_SCALE={spec}: expected 0.5 to 1.0");
    }
    scale
}

pub fn asked_upscaler() -> Option<u32> {
    let spec = std::env::var("XCL_UPSCALER").ok()?;
    let kind = spec.trim().parse::<u32>().ok().filter(|k| matches!(k, 6 | 7));
    if kind.is_none() {
        crate::xlog_warn!("loader: XCL_UPSCALER={spec}: expected 6 (title upscaler) or 7 (FidelityFX)");
    }
    kind
}

pub struct Site {
    pub at: usize,
    pub what: &'static str,
    pub bytes: &'static [u8],
}

pub const SET_SCREEN_SIZE: usize = 0x2478130;
pub const SCREEN_HEIGHT: usize = 0xa9438d0;
pub const HEIGHT_CHANGED: usize = 0xd8c3a0;

pub const OUTPUT: Site = Site {
    at: 0x224352b,
    what: "settings' output size applied",
    bytes: &[
        0x8b, 0x0d, 0x9b, 0x03, 0x70, 0x08, 0x8b, 0x86, 0x04, 0x02, 0x00, 0x00, 0x39, 0xc1, 0x75, 0x0d, 0x8b, 0x96, 0x08, 0x02,
        0x00, 0x00, 0xe8, 0xea, 0x4b, 0x23, 0x00, 0xeb, 0x20, 0x8b, 0xbe, 0x08, 0x02, 0x00, 0x00, 0x8b, 0x2d, 0x7c, 0x03, 0x70,
        0x08, 0x89, 0xc1, 0x89, 0xfa, 0xe8, 0xd3, 0x4b, 0x23, 0x00, 0x39, 0xfd, 0x74, 0x07, 0xb1, 0x01, 0xe8, 0x38, 0x8e, 0xb4,
        0xfe,
    ],
};

pub const POOL_CHECK: Site = Site {
    at: 0x247af25,
    what: "back buffer pool's left-over check",
    bytes: &[0x48, 0x3d, 0xff, 0xff, 0x1f, 0x00, 0x76, 0x01, 0xcc],
};

pub fn pool_check_code() -> Vec<u8> {
    let mut c = POOL_CHECK.bytes[..8].to_vec();
    c[6] = 0xeb;
    c
}

pub const OWN_SCALE: Site = Site {
    at: 0x28f1e78,
    what: "the title's own upscaler's render scale",
    bytes: &[0xc7, 0x81, 0x90, 0x00, 0x00, 0x00, 0xab, 0xaa, 0x2a, 0x3f],
};

pub const FFX_SCALE: Site = Site {
    at: 0x28ef680,
    what: "FidelityFX's render scale",
    bytes: &[
        0x41, 0x56, 0x56, 0x57, 0x53, 0x48, 0x83, 0xec, 0x28, 0x44, 0x89, 0xcb, 0x4c, 0x89, 0xc6, 0x48, 0x89, 0xd7, 0x49, 0x89,
        0xce, 0xe8, 0xa6, 0x5a, 0x78, 0xfd, 0x0f, 0xb6, 0xc0, 0x41, 0x89, 0x86, 0x90, 0x00, 0x00, 0x00, 0x84, 0xc0, 0x75, 0x0a,
        0xc5, 0xfa, 0x10, 0x05, 0x28, 0x78, 0x6d, 0x03, 0xeb, 0x08, 0xc5, 0xfa, 0x10, 0x05, 0xa2, 0xe2, 0x86, 0x03, 0xc5, 0xf2,
        0x2a, 0x54, 0x24, 0x70, 0xc5, 0xf2, 0x2a, 0xcb, 0xc5, 0xfa, 0x59, 0xc9, 0xc5, 0xfa, 0x2c, 0xc1, 0x89, 0x07, 0xc5, 0xfa,
        0x59, 0xc2, 0xc5, 0xfa, 0x2c, 0xc0, 0x89, 0x06, 0x48, 0x83, 0xc4, 0x28, 0x5b, 0x5f, 0x5e, 0x41, 0x5e, 0xc3, 0xcc, 0xcc,
        0xcc, 0xcc, 0xcc, 0xcc,
    ],
};

pub const PICKER: Site = Site {
    at: 0x7e93f0,
    what: "upscaler picker",
    bytes: &[
        0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 0x6d, 0xf0, 0x3a, 0x07, 0x00, 0x75, 0x09, 0x83, 0x3d, 0x68, 0xf0, 0x3a, 0x07, 0x00,
        0x74, 0x28, 0xe8, 0x35, 0xbd, 0x88, 0xff, 0x8a, 0x0d, 0x5b, 0xf0, 0x3a, 0x07, 0x80, 0xe1, 0x40, 0x80, 0xf9, 0x01, 0xb9,
        0x07, 0x00, 0x00, 0x00, 0x83, 0xd9, 0x00, 0x84, 0xc0, 0xb8, 0x06, 0x00, 0x00, 0x00, 0x0f, 0x45, 0xc1, 0x48, 0x83, 0xc4,
        0x28, 0xc3,
    ],
};

fn rel(to: usize, next: usize) -> [u8; 4] {
    ((to as i64 - next as i64) as i32).to_le_bytes()
}

pub fn output_code(width: u32, height: u32) -> Vec<u8> {
    let at = OUTPUT.at;
    let mut c = Vec::with_capacity(OUTPUT.bytes.len());
    c.push(0xb8);
    c.extend(width.to_le_bytes());
    c.extend([0x89, 0x86, 0x04, 0x02, 0x00, 0x00]);
    c.push(0xbf);
    c.extend(height.to_le_bytes());
    c.extend([0x89, 0xbe, 0x08, 0x02, 0x00, 0x00]);
    c.extend([0x8b, 0x2d]);
    c.extend(rel(SCREEN_HEIGHT, at + c.len() + 4));
    c.extend([0x89, 0xc1, 0x89, 0xfa]);
    c.push(0xe8);
    c.extend(rel(SET_SCREEN_SIZE, at + c.len() + 4));
    let end = OUTPUT.bytes.len();
    c.extend([0x39, 0xfd, 0x74, (end - (c.len() + 4)) as u8]);
    c.extend([0xb1, 0x01, 0xe8]);
    c.extend(rel(HEIGHT_CHANGED, at + c.len() + 4));
    c.extend([0xeb, (end - (c.len() + 2)) as u8]);
    c.resize(end, 0xcc);
    c
}

pub fn own_scale_code(scale: f32) -> Vec<u8> {
    let mut c = OWN_SCALE.bytes[..6].to_vec();
    c.extend(scale.to_le_bytes());
    c
}

pub fn ffx_scale_code(scale: f32) -> Vec<u8> {
    const LOADS: [usize; 2] = [0x28, 0x32];
    const SLOT: usize = 0x64;
    let mut c = FFX_SCALE.bytes.to_vec();
    c[SLOT..SLOT + 4].copy_from_slice(&scale.to_le_bytes());
    for l in LOADS {
        c[l + 4..l + 8].copy_from_slice(&((SLOT - (l + 8)) as i32).to_le_bytes());
    }
    c
}

pub fn picker_code(kind: u32) -> Vec<u8> {
    let mut c = vec![0xb8];
    c.extend(kind.to_le_bytes());
    c.push(0xc3);
    c
}

pub unsafe fn matches(base: usize, size: usize, site: &Site) -> Patched {
    if site.at.checked_add(site.bytes.len()).is_none_or(|end| end > size) {
        return Patched::Failed("outside the image");
    }
    let there = unsafe { std::slice::from_raw_parts((base + site.at) as *const u8, site.bytes.len()) };
    if there == site.bytes { Patched::Done } else { Patched::Differs }
}

#[cfg(windows)]
pub unsafe fn write(base: usize, size: usize, site: &Site, code: &[u8]) -> Patched {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    if code.len() > site.bytes.len() {
        return Patched::Failed("replacement longer than expected");
    }
    let found = unsafe { matches(base, size, site) };
    if found != Patched::Done {
        return found;
    }
    let at = base + site.at;
    let mut old = 0;
    unsafe {
        if VirtualProtect(at as *const _, code.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return Patched::Failed("VirtualProtect");
        }
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
        VirtualProtect(at as *const _, code.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), at as *const _, code.len());
    }
    Patched::Done
}

#[cfg(windows)]
pub unsafe fn apply_asked(base: usize, size: usize) {
    let mut asked: Vec<(&Site, Vec<u8>, String)> = Vec::new();
    if let Some((w, h)) = asked_output() {
        asked.push((&OUTPUT, output_code(w, h), format!("{w}x{h}")));
        if (w, h) != (3840, 2160) {
            asked.push((&POOL_CHECK, pool_check_code(), format!("{w}x{h}")));
        }
    }
    if let Some(s) = asked_render_scale() {
        asked.push((&OWN_SCALE, own_scale_code(s), format!("{s}")));
    }
    if let Some(s) = asked_ffx_scale() {
        asked.push((&FFX_SCALE, ffx_scale_code(s), format!("{s}")));
    }
    if let Some(kind) = asked_upscaler() {
        asked.push((&PICKER, picker_code(kind), format!("returns {kind}")));
    }
    for (site, code, value) in asked {
        let done = unsafe { write(base, size, site, &code) };
        let offset = format!("{:#x}", site.at);
        match done {
            Patched::Done => crate::xlog_info!("loader: resolution {} ({value}) at title+{offset}", site.what),
            Patched::Differs => crate::xlog_warn!("loader: resolution {} at title+{offset}: bytes differ (other build?)", site.what),
            Patched::Failed(why) => crate::xlog_warn!("loader: resolution {} at title+{offset}: not patched ({why})", site.what),
        }
    }
}

#[cfg(not(windows))]
pub unsafe fn apply_asked(_base: usize, _size: usize) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_parse() {
        assert_eq!(parse_output("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_render_scale("1"), Some(1.0));
        assert_eq!(picker_code(7), [0xb8, 7, 0, 0, 0, 0xc3]);
        assert_eq!(pool_check_code(), [0x48, 0x3d, 0xff, 0xff, 0x1f, 0x00, 0xeb, 0x01]);
    }
}