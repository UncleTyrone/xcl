//! Belobug renderer port 1 (`XCL_PORT_CONSTANTS`) — NVIDIA stub.
//!
//! Main/amd place title constants via a second view of the physical-page section
//! (`OpenExistingHeapFromAddress1`). Until that placement path is mirrored here, call sites
//! that only need the latch + touch filter treat every range as touched when the latch is on.

use super::port;

pub fn on() -> bool {
    port::constants()
}

/// Touch filter for prelude fills: accept all ranges while placement is stubbed.
pub fn touches_range(_src: usize, _len: usize) -> bool {
    true
}
