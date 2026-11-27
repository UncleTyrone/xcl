//! Optional boot-time diagnostic threads (CGBJ `run` diagnostics parity).

use std::path::{Path, PathBuf};
use std::time::Duration;

struct Word {
    name: String,
    offset: usize,
    bytes: usize,
}

fn parse_image_words(text: &str) -> Vec<Word> {
    text.split(',')
        .filter_map(|part| {
            let (name, rest) = part.trim().split_once('=')?;
            let (offset, bytes) = rest.split_once(':').unwrap_or((rest, "8"));
            let offset = usize::from_str_radix(offset.trim().trim_start_matches("0x"), 16).ok()?;
            let bytes = bytes.trim().parse().ok().filter(|n| matches!(n, 1 | 2 | 4 | 8))?;
            Some(Word { name: name.trim().to_owned(), offset, bytes })
        })
        .collect()
}

fn read_image_word(base: usize, word: &Word, size: usize) -> Option<u64> {
    if word.offset.checked_add(word.bytes).is_none_or(|end| end > size) {
        return None;
    }
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut value = [0u8; 8];
    // SAFETY: kernel copies at most eight bytes; fails where nothing is mapped.
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            (base + word.offset) as *const _,
            value.as_mut_ptr().cast(),
            word.bytes,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then(|| u64::from_le_bytes(value))
}

/// `XCL_IMAGE_WATCH=used=98d26d8:8,failed=98d26d0:1` — log named fields once per second when they change.
/// Also accepts `VICTORY_IMAGE_WATCH` for CGBJ parity.
pub fn start_image_watch(base: usize, size: usize) {
    let text = std::env::var("XCL_IMAGE_WATCH")
        .or_else(|_| std::env::var("VICTORY_IMAGE_WATCH"));
    let Ok(text) = text else {
        return;
    };
    let words = parse_image_words(&text);
    if base == 0 || words.is_empty() {
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        crate::xlog_info!("XCL_IMAGE_WATCH: {} word(s)", words.len());
        let _ = std::thread::Builder::new().name("xcl-image-watch".into()).spawn(move || {
            let mut last = Vec::new();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let now: Vec<Option<u64>> = words.iter().map(|w| read_image_word(base, w, size)).collect();
                if now == last {
                    continue;
                }
                let line: Vec<String> = words
                    .iter()
                    .zip(&now)
                    .map(|(w, v)| match v {
                        Some(v) => format!("{}={v:#x}", w.name),
                        None => format!("{}=?", w.name),
                    })
                    .collect();
                crate::xlog_info!("title image: {}", line.join(" "));
                last = now;
            }
        });
    });
}

/// `XCL_TITLE_WATCH=0xrva,…` — poll u32 at RVAs every 5 ms (CGBJ `VICTORY_WATCH`; not the write hook `XCL_WATCH`).
pub fn start_title_watch(base: usize, size: usize) {
    let Ok(list) = std::env::var("XCL_TITLE_WATCH") else {
        return;
    };
    let rvas: Vec<usize> = list
        .split(',')
        .filter_map(|r| usize::from_str_radix(r.trim().trim_start_matches("0x"), 16).ok())
        .filter(|&rva| rva + 4 <= size)
        .collect();
    if rvas.is_empty() {
        return;
    }
    crate::xlog_info!("XCL_TITLE_WATCH: {} slot(s)", rvas.len());
    std::thread::spawn(move || {
        let mut seen = vec![None; rvas.len()];
        loop {
            for (&rva, slot) in rvas.iter().zip(&mut seen) {
                let value = unsafe { ((base + rva) as *const u32).read_volatile() };
                if *slot != Some(value) {
                    crate::xlog_info!("title+{rva:#x} = {value:#x}");
                    *slot = Some(value);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}

/// `XCL_POKE=rva:byte,…` — rewrite bytes every 500 ms (30 s idle watchdog aids).
pub fn start_poke(base: usize, size: usize) {
    let Ok(list) = std::env::var("XCL_POKE") else {
        return;
    };
    let hex = |s: &str| usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
    let pokes: Vec<(usize, u8)> = list
        .split(',')
        .filter_map(|p| p.split_once(':'))
        .filter_map(|(rva, byte)| Some((hex(rva)?, u8::try_from(hex(byte)?).ok()?)))
        .filter(|&(rva, _)| rva < size)
        .collect();
    if pokes.is_empty() {
        return;
    }
    crate::xlog_info!("XCL_POKE: {} site(s)", pokes.len());
    std::thread::spawn(move || loop {
        for &(rva, byte) in &pokes {
            unsafe { ((base + rva) as *mut u8).write_volatile(byte) };
        }
        std::thread::sleep(Duration::from_millis(500));
    });
}

static CRASH_RANGES: std::sync::OnceLock<Vec<(usize, usize, PathBuf)>> = std::sync::OnceLock::new();

/// `XCL_CRASH_SAVE=rva:len,…` — on unhandled exception, write each range to `cache/crash-<rva>.bin`.
pub fn register_crash_save(base: usize, size: usize, cache_dir: &Path) {
    let ranges = std::env::var("XCL_CRASH_SAVE").unwrap_or_default();
    if ranges.trim().is_empty() {
        return;
    }
    let hex = |s: &str| usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
    let list: Vec<(usize, usize, PathBuf)> = ranges
        .split(',')
        .filter_map(|r| r.split_once(':'))
        .filter_map(|(rva, len)| Some((hex(rva)?, hex(len)?)))
        .filter(|&(rva, len)| rva.checked_add(len).is_some_and(|end| end <= size))
        .map(|(rva, len)| (base + rva, len, cache_dir.join(format!("crash-{rva:x}.bin"))))
        .collect();
    if list.is_empty() {
        return;
    }
    crate::xlog_info!("XCL_CRASH_SAVE: {} range(s) under {}", list.len(), cache_dir.display());
    let _ = CRASH_RANGES.set(list);
}

pub fn write_crash_snapshots() {
    let Some(ranges) = CRASH_RANGES.get() else { return };
    for &(addr, len, ref path) in ranges {
        let mut buf = vec![0u8; len];
        let ok = unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                addr as *const _,
                buf.as_mut_ptr().cast(),
                len,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            let _ = std::fs::write(path, &buf);
            crate::xlog_error!("XCL_CRASH_SAVE: wrote {} B to {}", len, path.display());
        }
    }
}

pub fn start_after_main(base: usize, size: usize, cache_dir: &Path) {
    register_crash_save(base, size, cache_dir);
    start_image_watch(base, size);
    start_title_watch(base, size);
    start_poke(base, size);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_watch_words_parse() {
        let all = parse_image_words("used=98d26d8:8, failed = 0x10:1,odd=3:3,none");
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].name.as_str(), all[0].offset, all[0].bytes), ("used", 0x98d_26d8, 8));
        assert_eq!((all[1].name.as_str(), all[1].offset, all[1].bytes), ("failed", 0x10, 1));
    }

    #[test]
    fn image_watch_reads_via_process_memory() {
        let memory = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let base = memory.as_ptr() as usize;
        let word = |offset, bytes| Word {
            name: String::new(),
            offset,
            bytes,
        };
        assert_eq!(read_image_word(base, &word(0, 4), 8), Some(0x4433_2211));
        assert_eq!(read_image_word(base, &word(2, 1), 8), Some(0x33));
        assert_eq!(read_image_word(base, &word(0, 8), 4), None);
    }
}