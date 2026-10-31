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

use serde_json::{Value, json};

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

/// Lifecycle event kind. Only progress and heartbeat may coalesce or
/// drop under flood; authoritative transitions never drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Ready,
    Degraded,
    Failed,
    Progress,
    Heartbeat,
}

/// Reports whether the kind may coalesce or drop under flood.
#[must_use]
pub fn may_coalesce(kind: EventKind) -> bool {
    matches!(kind, EventKind::Progress | EventKind::Heartbeat)
}

/// Refusal of an outbox admission past its bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboxRefusal {
    DataCapExceeded,
    ControlCapExceeded,
}

/// Transmit-side queue accounting with the perspective rule: data
/// writes count against the peer's receive cap, control traffic rides
/// a priority lane exempt from data caps within its own bound. Writes
/// block only up to the bounded drain wait (enforced by the run loop
/// against the clock), then fail rather than queue unboundedly.
#[derive(Debug)]
pub struct Outbox {
    data_cap_bytes: usize,
    control_cap: usize,
    data_bytes: usize,
    control_pending: usize,
}

impl Outbox {
    /// Creates an outbox with the negotiated caps.
    #[must_use]
    pub fn new(data_cap_bytes: usize, control_cap: usize) -> Self {
        Self {
            data_cap_bytes,
            control_cap,
            data_bytes: 0,
            control_pending: 0,
        }
    }

    /// Admits a data write of `len` bytes, failing past the cap.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxRefusal::DataCapExceeded`] when admission
    /// would exceed the data cap.
    pub fn push_data(&mut self, len: usize) -> Result<(), OutboxRefusal> {
        if self.data_bytes.saturating_add(len) > self.data_cap_bytes {
            return Err(OutboxRefusal::DataCapExceeded);
        }
        self.data_bytes = self.data_bytes.saturating_add(len);
        Ok(())
    }

    /// Admits one control signal, failing past the control bound.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxRefusal::ControlCapExceeded`] when the control
    /// lane is full; the caller fences instead of queueing.
    pub fn push_control(&mut self) -> Result<(), OutboxRefusal> {
        if self.control_pending >= self.control_cap {
            return Err(OutboxRefusal::ControlCapExceeded);
        }
        self.control_pending = self.control_pending.saturating_add(1);
        Ok(())
    }

    /// Releases `len` data bytes after the write leaves the outbox.
    pub fn release_data(&mut self, len: usize) {
        self.data_bytes = self.data_bytes.saturating_sub(len);
    }

    /// Releases one control signal after admission resolves.
    pub fn release_control(&mut self) {
        self.control_pending = self.control_pending.saturating_sub(1);
    }
}

/// Liveness tracked from valid frames. Expiry follows the same local
/// path as a disconnect: cancel resolvers, invalidate pending state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Liveness {
    last_valid_frame_ms: u64,
    timeout_ms: u64,
}

impl Liveness {
    /// Starts liveness at the handshake reading with the negotiated
    /// peer timeout.
    #[must_use]
    pub fn new(now_ms: u64, timeout_ms: u64) -> Self {
        Self {
            last_valid_frame_ms: now_ms,
            timeout_ms,
        }
    }

    /// Records a valid frame arrival.
    pub fn observe(&mut self, now_ms: u64) {
        self.last_valid_frame_ms = now_ms;
    }

    /// Reports whether the peer is silent past the timeout.
    #[must_use]
    pub fn is_expired(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_valid_frame_ms) >= self.timeout_ms
    }
}

/// Builds the `relay.ping` reply: the empty object. Presence of the
/// reply is the liveness proof; it carries no payload.
#[must_use]
pub fn ping_reply() -> Value {
    json!({})
}
