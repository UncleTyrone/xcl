//! Lightweight Chrome-trace timeline (`XCL_TIMELINE=<file>`).
//!
//! Records presents, submits, and named spans from the first present after
//! `XCL_TIMELINE_AT` seconds (default 60) for `XCL_TIMELINE_FRAMES` frames
//! (default 3). Writes `<file>` (JSON) and `<file>.txt` summary.
//! Full wait-hook wrapping remains latch-adjacent; this is the real event sink.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Settings {
    file: PathBuf,
    at_secs: f64,
    frames: u64,
}

fn settings() -> Option<&'static Settings> {
    static S: OnceLock<Option<Settings>> = OnceLock::new();
    S.get_or_init(|| {
        let raw = std::env::var("XCL_TIMELINE")
            .ok()
            .or_else(|| std::env::var("VICTORY_TIMELINE").ok())
            .filter(|v| !v.is_empty() && v != "0")?;
        // Forms: `<file>` | `<secs>,<frames>,<file>` | `<secs>+…,<frames>,<file>`
        let (at_secs, frames, file) = if let Some((left, file)) = raw.rsplit_once(',') {
            let file = PathBuf::from(file.trim());
            if let Some((at, fr)) = left.rsplit_once(',') {
                let at = at.split('+').next().unwrap_or(at).trim().parse().unwrap_or(60.0);
                let frames = fr.trim().parse().unwrap_or(3u64);
                (at, frames, file)
            } else {
                let at = std::env::var("XCL_TIMELINE_AT")
                    .ok()
                    .or_else(|| std::env::var("VICTORY_TIMELINE_AT").ok())
                    .and_then(|v| v.split(',').next()?.trim().parse().ok())
                    .unwrap_or(60.0);
                let frames = left.trim().parse().unwrap_or(3);
                (at, frames, file)
            }
        } else {
            let at = std::env::var("XCL_TIMELINE_AT")
                .ok()
                .or_else(|| std::env::var("VICTORY_TIMELINE_AT").ok())
                .and_then(|v| v.split(',').next()?.trim().parse().ok())
                .unwrap_or(60.0);
            let frames = std::env::var("XCL_TIMELINE_FRAMES")
                .ok()
                .or_else(|| std::env::var("VICTORY_TIMELINE_FRAMES").ok())
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(3);
            (at, frames, PathBuf::from(raw))
        };
        crate::xlog_info!(
            "D3D12.X: XCL_TIMELINE → {} after {at_secs}s for {frames} frame(s)",
            file.display()
        );
        // Touch latches so boot scrape sees them consumed.
        let _ = crate::hle::d3d12x::bench_env::timeline();
        Some(Settings { file, at_secs, frames })
    })
    .as_ref()
}

/// Whether the timeline is actively recording.
pub(crate) fn on() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

static RECORDING: AtomicBool = AtomicBool::new(false);
static LEFT: AtomicU64 = AtomicU64::new(0);
static BEGAN: OnceLock<Instant> = OnceLock::new();

struct Event {
    name: String,
    ph: char,
    ts_us: u64,
    tid: u32,
    args: String,
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

fn now_us() -> u64 {
    let t0 = BEGAN.get_or_init(Instant::now);
    t0.elapsed().as_micros() as u64
}

fn tid() -> u32 {
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

fn push(name: impl Into<String>, ph: char, args: impl Into<String>) {
    if !on() {
        return;
    }
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).push(Event {
        name: name.into(),
        ph,
        ts_us: now_us(),
        tid: tid(),
        args: args.into(),
    });
}

/// A named span: Instant → Drop ends it.
pub(crate) struct Span {
    name: &'static str,
    active: bool,
}

impl Span {
    pub(crate) fn next(&mut self, name: &'static str) {
        if self.active {
            push(self.name, 'E', "");
        }
        self.name = name;
        if on() {
            push(name, 'B', "");
            self.active = true;
        }
    }

    pub(crate) fn end(self) {}
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.active {
            push(self.name, 'E', "");
        }
    }
}

pub(crate) fn span(name: &'static str) -> Span {
    let active = on();
    if active {
        push(name, 'B', "");
    }
    Span { name, active }
}

pub(crate) fn presented(frame: u64) {
    let Some(s) = settings() else { return };
    let began = *BEGAN.get_or_init(Instant::now);

    if on() {
        push("present", 'i', format!("\"frame\":{frame}"));
        if LEFT.fetch_sub(1, Ordering::Relaxed) <= 1 {
            RECORDING.store(false, Ordering::Relaxed);
            flush(&s.file);
            super::stamps::timeline_restart();
        }
        return;
    }

    if began.elapsed().as_secs_f64() >= s.at_secs {
        LEFT.store(s.frames.max(1), Ordering::Relaxed);
        RECORDING.store(true, Ordering::Relaxed);
        EVENTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        super::stamps::timeline_ready();
        crate::xlog_info!(
            "D3D12.X: XCL_TIMELINE recording {} frame(s) → {}",
            s.frames,
            s.file.display()
        );
        push("present", 'i', format!("\"frame\":{frame}"));
    }
}

pub(crate) fn sent() {
    push("submit", 'i', "");
}

/// Frame-record / wire event (when timeline and `XCL_RECORD` overlap).
pub(crate) fn frame_record(name: &str, detail: impl Into<String>) {
    if !on() {
        return;
    }
    let d = detail.into();
    let args = if d.is_empty() {
        String::new()
    } else {
        format!("\"detail\":\"{}\"", d.replace('\\', "\\\\").replace('"', "\\\""))
    };
    push(name, 'i', args);
}

fn flush(file: &PathBuf) {
    use std::fmt::Write as _;
    let events = std::mem::take(&mut *EVENTS.lock().unwrap_or_else(|e| e.into_inner()));
    let stamps = super::stamps::timeline_stamps();
    let mut json = String::from("{\"traceEvents\":[\n");
    let mut first = true;
    for e in &events {
        if !first {
            json.push_str(",\n");
        }
        first = false;
        let args = if e.args.is_empty() {
            String::new()
        } else {
            format!(",\"args\":{{{}}}", e.args)
        };
        let _ = write!(
            json,
            "{{\"name\":{},\"cat\":\"xcl\",\"ph\":\"{}\",\"ts\":{},\"pid\":1,\"tid\":{}{}}}",
            serde_json::to_string(&e.name).unwrap_or_else(|_| "\"?\"".into()),
            e.ph,
            e.ts_us,
            e.tid,
            args
        );
    }
    for (i, (a, b)) in stamps.iter().enumerate() {
        if *a == 0 && *b == 0 {
            continue;
        }
        if !first {
            json.push_str(",\n");
        }
        first = false;
        let _ = write!(
            json,
            "{{\"name\":\"gpu\",\"cat\":\"card\",\"ph\":\"X\",\"ts\":{},\"dur\":{},\"pid\":1,\"tid\":0,\"args\":{{\"place\":{}}}}}",
            a / 10,
            b.saturating_sub(*a) / 10,
            i
        );
    }
    json.push_str("\n]}\n");
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(file, &json) {
        Ok(()) => crate::xlog_info!("D3D12.X: XCL_TIMELINE wrote {} ({} events)", file.display(), events.len()),
        Err(e) => crate::xlog_warn!("D3D12.X: XCL_TIMELINE write {}: {e}", file.display()),
    }
    let txt = {
        let mut p = file.clone();
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("timeline");
        p.set_file_name(format!("{name}.txt"));
        p
    };
    let mut summary = format!("XCL timeline: {} events, {} card stamps\n", events.len(), stamps.len());
    for e in &events {
        let _ = writeln!(summary, "{:>10} us  tid {}  {} {}", e.ts_us, e.tid, e.ph, e.name);
    }
    if std::fs::write(&txt, summary).is_ok() {
        crate::xlog_info!("D3D12.X: XCL_TIMELINE summary {}", txt.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_is_noop_when_off() {
        assert!(!on());
        let mut s = span("test");
        s.next("other");
        s.end();
    }
}
