//! Lifecycle events, ping liveness, and the diagnostic sink.
//!
//! Emits ready/degraded/failed events with namespaced typed codes
//! without dictating supervision outcomes; coalesces or drops only
//! progress/heartbeat under flood while authoritative transitions are
//! never dropped. Answers `relay.ping` with the empty-object result and
//! tracks liveness from valid frames. Owns the local diagnostic and
//! inscription sink for shed gate-drops and cleanup records — structured
//! records, never stdout, so diagnostics cannot corrupt the frame stream.

use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

/// Monotonic millisecond source for adapter-side waits.
///
/// All prime-waits, liveness tracking, and watchdog-adjacent local timing
/// read through this trait so replay runs on virtual time. The live
/// executable feeds it from a monotonic system source; tests and the
/// conformance harness feed it from [`ManualClock`].
pub trait Clock {
    /// Returns the current reading in milliseconds since an unspecified
    /// epoch. Only differences between readings are meaningful.
    fn now_ms(&self) -> u64;
}

/// System-backed [`Clock`] for the live executable.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Manually advanced [`Clock`] for tests, replay, and the conformance
/// harness.
///
/// Time moves only through [`ManualClock::advance`], so an input trace
/// that includes virtual-clock advances replays deterministically. The
/// clock saturates at `u64::MAX` rather than wrapping.
#[derive(Debug, Default)]
pub struct ManualClock {
    now_ms: Cell<u64>,
}

impl ManualClock {
    /// Creates a clock reading `start_ms`.
    #[must_use]
    pub fn new(start_ms: u64) -> Self {
        Self {
            now_ms: Cell::new(start_ms),
        }
    }

    /// Advances the reading by `delta_ms`, saturating at `u64::MAX`.
    pub fn advance(&self, delta_ms: u64) {
        self.now_ms.set(self.now_ms.get().saturating_add(delta_ms));
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.get()
    }
}
