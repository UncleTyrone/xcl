//! Host GPU diagnostics (CGBJ `victory-gpu/host/*` parity surface).
//!
//! Real consumers for PANEL / CAPTURE / TIMELINE / FAULT_BOOK and the
//! panel / upcheck / torn / traffic / stamps / spans / place / aside spine.
//! GPL-2 reimplementation from CGBJ study (not a paste).
//!
//! `faultbook` and `panel` live as sibling modules (already wired from present);
//! this package owns the rest of the host spine and the present/submit ticks.

pub(crate) mod aside;
pub(crate) mod capture;
pub(crate) mod place;
pub(crate) mod spans;
pub(crate) mod stamps;
pub(crate) mod timeline;
pub(crate) mod torn;
pub(crate) mod traffic;
pub(crate) mod upcheck;

/// Frame-rate / present tick: capture world, timeline; panel/faultbook already called by present.
pub(crate) fn on_present(frame: u64, draws_this_frame: u64) {
    capture::frame_drew(frame, draws_this_frame);
    timeline::presented(frame);
}

/// Every ~300 presents: log host diagnostic lines.
pub(crate) fn note_frame(frames: f64) {
    traffic::line(frames);
    spans::line(frames);
    place::line(frames);
    torn::line(frames);
    upcheck::line(frames);
    aside::line(frames);
}

/// Submit about to go to the driver: stamps / spans / timeline.
pub(crate) fn before_submit() -> Option<spans::SpanTicket> {
    super::shadow::let_go_idle();
    timeline::sent();
    spans::begin()
}

/// Submit returned from the driver.
pub(crate) fn after_submit(ticket: Option<spans::SpanTicket>) {
    spans::end(ticket);
}
