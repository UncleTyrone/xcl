//! Title frame-rate reads patched at load (`XCL_FRAME_INTERVAL_MS`, off unless set).
//! Reimplements victory-loader `frame_rate` for game_scarlett_master.exe 1.0.2.1 (byte compare first).

/// Loader switch: interval in milliseconds for pairing with HLE `frame.rs` (0 = uncapped). `None` if unset.
pub fn asked_interval_ms() -> Option<f64> {
    let spec = std::env::var("XCL_FRAME_INTERVAL_MS").ok()?;
    let ms = parse_interval_ms(&spec);
    if ms.is_none() {
        crate::xlog_warn!("loader: XCL_FRAME_INTERVAL_MS={spec}: expected 0..=1000 ms");
    }
    ms
}

pub fn parse_interval_ms(spec: &str) -> Option<f64> {
    spec.trim().parse::<f64>().ok().filter(|ms| ms.is_finite() && (0.0..=1000.0).contains(ms))
}

/// Title rate for `interval_ms`: derived from ms unless `XCL_TITLE_FPS` is set (`off` → no patch).
pub fn title_rate(interval_ms: f64, title_fps: Option<&str>) -> Option<u32> {
    match title_fps.map(str::trim) {
        Some("off") => None,
        Some(rate) => rate.parse::<u32>().ok().filter(|r| *r <= 1000),
        None if interval_ms <= 0.0 => Some(0),
        None => Some(((1000.0 / interval_ms).round() as u32).clamp(1, 1000)),
    }
}

pub fn title_frame_us(rate: u32) -> u32 {
    let clamped = if rate == 0 || rate >= 1000 { 1000 } else { rate };
    (1_000_000.0_f32 / clamped as f32) as u32
}

pub fn title_interval_us(rate: u32, display_120: bool) -> u32 {
    const TABLE: [[u32; 5]; 2] = [[16667, 16667, 33333, 50000, 66667], [8333, 8333, 16667, 25000, 33333]];
    let fps = (1_000_000.0_f32 / title_frame_us(rate) as f32) as u32 & 0xffff;
    let display = if display_120 { 120 } else { 60 };
    let index = if fps <= display { (display / fps.max(1)).min(4) } else { 1 };
    TABLE[usize::from(display_120)][index as usize]
}

pub struct Site {
    pub at: usize,
    pub kind: Kind,
    pub bytes: &'static [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Load,
    SimRate,
}

pub const SITES: [Site; 3] = [
    Site {
        at: 0x1e0fc62,
        kind: Kind::Load,
        bytes: &[
            0x8b, 0x87, 0x10, 0x02, 0x00, 0x00, 0xba, 0xe8, 0x03, 0x00, 0x00, 0xc5, 0xfa, 0x10, 0x0d, 0x9f, 0xa1, 0x34, 0x04, 0xb9, 0xe8,
            0x03, 0x00, 0x00, 0x3d, 0xe8, 0x03, 0x00, 0x00, 0x0f, 0x42, 0xd0, 0x85, 0xc0, 0x0f, 0x44, 0xd1, 0xc5, 0xea, 0x2a, 0xc2, 0xc5,
            0xf2, 0x5e, 0xc0, 0xc4, 0xe1, 0xfa, 0x2c, 0xc0, 0x89, 0x05, 0x12, 0x88, 0xae, 0x07,
        ],
    },
    Site {
        at: 0x224359a,
        kind: Kind::Load,
        bytes: &[
            0x8b, 0x86, 0x10, 0x02, 0x00, 0x00, 0xba, 0xe8, 0x03, 0x00, 0x00, 0xc5, 0xfa, 0x10, 0x0d, 0x67, 0x68, 0xf1, 0x03, 0xb9, 0xe8,
            0x03, 0x00, 0x00, 0x3d, 0xe8, 0x03, 0x00, 0x00, 0x0f, 0x42, 0xd0, 0x85, 0xc0, 0x0f, 0x44, 0xd1, 0xb1, 0xff, 0xc5, 0xea, 0x2a,
            0xc2, 0xc5, 0xf2, 0x5e, 0xc0, 0xc4, 0xe1, 0xfa, 0x2c, 0xc0, 0x89, 0x05, 0xd8, 0x4e, 0x6b, 0x07,
        ],
    },
    Site {
        at: 0x2241fdb,
        kind: Kind::SimRate,
        bytes: &[
            0x83, 0xb8, 0x10, 0x02, 0x00, 0x00, 0x00, 0x74, 0x12, 0xe8, 0xc7, 0xa1, 0xe0, 0x02, 0x8b, 0x80, 0x10, 0x02, 0x00, 0x00, 0xc4,
            0xe1, 0xfa, 0x2a, 0xc0,
        ],
    },
];

pub const FRAME_US: usize = 0x98f84ac;

pub fn replacement(kind: Kind, rate: u32) -> Vec<u8> {
    match kind {
        Kind::Load => {
            let r = rate.to_le_bytes();
            vec![0xb8, r[0], r[1], r[2], r[3], 0x90]
        }
        Kind::SimRate => {
            let r = (if rate == 0 { 1000 } else { rate }).to_le_bytes();
            vec![0xb8, r[0], r[1], r[2], r[3], 0xeb, 0x0d]
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Patched {
    Done,
    Differs,
    Failed(&'static str),
}

#[cfg(windows)]
pub unsafe fn apply(base: usize, size: usize, rate: u32, sites: &[Site]) -> Vec<Patched> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    sites
        .iter()
        .map(|site| {
            let code = replacement(site.kind, rate);
            let len = code.len();
            let done = (|| {
                if site.bytes.len() < len || site.at.checked_add(site.bytes.len()).is_none_or(|end| end > size) {
                    return Patched::Failed("outside the image");
                }
                let at = base + site.at;
                let there = unsafe { std::slice::from_raw_parts(at as *const u8, site.bytes.len()) };
                if there != site.bytes {
                    return Patched::Differs;
                }
                let mut old = 0;
                unsafe {
                    if VirtualProtect(at as *const _, len, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                        return Patched::Failed("VirtualProtect");
                    }
                    std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, len);
                    VirtualProtect(at as *const _, len, old, &mut old);
                    FlushInstructionCache(GetCurrentProcess(), at as *const _, len);
                }
                Patched::Done
            })();
            match done {
                Patched::Done => crate::xlog_info!(
                    "loader: frame rate site {:#x} {:?} → rate {rate} (XCL_FRAME_INTERVAL_MS)",
                    site.at,
                    site.kind
                ),
                Patched::Differs => crate::xlog_warn!("loader: frame rate site {:#x} bytes differ (other build?)", site.at),
                Patched::Failed(why) => crate::xlog_warn!("loader: frame rate site {:#x} not patched: {why}", site.at),
            }
            done
        })
        .collect()
}

#[cfg(windows)]
pub unsafe fn apply_if_asked(base: usize, size: usize) {
    let Some(ms) = asked_interval_ms() else { return };
    let title_fps = std::env::var("XCL_TITLE_FPS").ok();
    let Some(rate) = title_rate(ms, title_fps.as_deref()) else {
        crate::xlog_info!("loader: frame clock {ms} ms; title FPS left at 30 (XCL_TITLE_FPS=off)");
        return;
    };
    let done = apply(base, size, rate, &SITES);
    let patched = done.iter().filter(|d| **d == Patched::Done).count();
    crate::xlog_info!(
        "loader: frame clock {ms} ms; title rate {rate} ({patched}/{} sites); title interval {} µs @60 Hz",
        SITES.len(),
        title_interval_us(rate, false)
    );
}

#[cfg(not(windows))]
pub unsafe fn apply_if_asked(_base: usize, _size: usize) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_and_rates() {
        assert_eq!(parse_interval_ms("16.667"), Some(16.667));
        assert_eq!(title_rate(16.667, None), Some(60));
        assert_eq!(title_rate(16.667, Some("off")), None);
    }
}