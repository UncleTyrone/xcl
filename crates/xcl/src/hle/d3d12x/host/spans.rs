//! Card-span timing around submissions (`XCL_CARD_SPANS=1`).
//!
//! Measures wall time from before `ExecuteCommandLists` to after the driver
//! call returns (queue accept). Full GPU query-heap stamps remain a follow-on;
//! this replaces the latch-only log with real per-submit accounting.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::super::session;

pub(crate) fn on() -> bool {
    session::card_spans()
}

#[derive(Default)]
struct Sums {
    count: u64,
    busy_us: u64,
    between_us: u64,
    longest_us: u64,
    last_end: Option<Instant>,
}

static SUMS: std::sync::Mutex<Sums> = std::sync::Mutex::new(Sums {
    count: 0,
    busy_us: 0,
    between_us: 0,
    longest_us: 0,
    last_end: None,
});

static BEGAN: AtomicU64 = AtomicU64::new(0);

/// Opaque ticket from [`begin`].
pub(crate) struct SpanTicket {
    start: Instant,
    stamp_at: Option<usize>,
}

pub(crate) fn begin() -> Option<SpanTicket> {
    if !on() && !super::timeline::on() {
        return None;
    }
    let stamp_at = super::stamps::take();
    if !on() && stamp_at.is_none() {
        return None;
    }
    Some(SpanTicket {
        start: Instant::now(),
        stamp_at,
    })
}

pub(crate) fn end(ticket: Option<SpanTicket>) {
    let Some(t) = ticket else { return };
    super::stamps::finish(t.stamp_at);
    if !on() {
        return;
    }
    let took = t.start.elapsed().as_micros() as u64;
    let mut s = SUMS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(last) = s.last_end {
        s.between_us += last.elapsed().as_micros() as u64;
    }
    s.count += 1;
    s.busy_us += took;
    s.longest_us = s.longest_us.max(took);
    s.last_end = Some(Instant::now());
    BEGAN.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn line(frames: f64) {
    if !on() {
        return;
    }
    let mut s = SUMS.lock().unwrap_or_else(|e| e.into_inner());
    if s.count == 0 {
        return;
    }
    let f = frames.max(1.0);
    let count = s.count as f64 / f;
    let busy = s.busy_us as f64 / 1e3 / f;
    let between = s.between_us as f64 / 1e3 / f;
    let longest = s.longest_us as f64 / 1e3;
    crate::xlog_info!(
        "D3D12.X: XCL_CARD_SPANS per frame: {count:.1} submits, {busy:.2} ms queue-accept busy, {between:.2} ms between, longest {longest:.2} ms"
    );
    *s = Sums::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_none_is_ok() {
        end(None);
    }
}
