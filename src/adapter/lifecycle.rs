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
use std::sync::LazyLock;
use std::time::Instant;

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

    /// Advances the reading by `delta_ms`. Fixture-driven virtual time
    /// consumes one scheduled advance per post-handshake frame, so
    /// timeout expiry in replay is driven only by trace events; the
    /// live clock ignores advances.
    fn advance(&self, _delta_ms: u64) {}
}

/// Monotonic millisecond source for adapter-side deadlines.
/// Backed by a steady clock anchored at first use: readings never go
/// backwards, unlike wall-clock time. The live executable feeds on
/// this; tests and replay use [`ManualClock`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

/// Process-start anchor for [`SystemClock`]. Lazily initialized on
/// first use; `elapsed` is monotonic by construction thereafter.
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        START.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
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
        Clock::advance(self, delta_ms);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.get()
    }

    fn advance(&self, delta_ms: u64) {
        self.now_ms.set(self.now_ms.get().saturating_add(delta_ms));
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

    /// Installs replacement caps while preserving outstanding
    /// usage counts: the handshakeHello request itself is already
    /// enqueued when the hello result installs the negotiated limits,
    /// so resetting to empty would strand those bytes (leaking cap)
    /// and let their late completions underflow the fresh counters.
    /// A tighter negotiated cap simply refuses new admissions until
    /// the outstanding bytes drain — bounded memory by refusal.
    pub fn set_caps(&mut self, data_cap_bytes: usize, control_cap: usize) {
        self.data_cap_bytes = data_cap_bytes;
        self.control_cap = control_cap;
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

    /// Reports an empty queue: nothing outstanding on either lane.
    /// The drain watchdog arms only while this is false.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data_bytes == 0 && self.control_pending == 0
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

    /// Returns the instant at which silence becomes expiry.
    #[must_use]
    pub fn deadline_ms(&self) -> u64 {
        self.last_valid_frame_ms.saturating_add(self.timeout_ms)
    }
}

/// Builds the `relay.ping` reply: the empty object. Presence of the
/// reply is the liveness proof; it carries no payload.
#[must_use]
pub fn ping_reply() -> Value {
    json!({})
}

/// Diagnostic sink writing JSON lines to stderr. Stdout stays
/// frame-only, so diagnostics never corrupt the frame stream.
pub struct StderrSink;

impl super::choice::DiagnosticSink for StderrSink {
    fn record(&mut self, entry: super::choice::DiagnosticEntry) {
        eprintln!(
            "{}",
            json!({
                "code": entry.code,
                "detail": entry.detail,
            })
        );
    }
}

use super::Adapter;
use super::binding::{CallKind, Direction};
use super::{EXIT_CLEAN, EXIT_REFUSAL};

impl Adapter<'_> {
    pub(crate) fn handle_ping(&mut self, envelope: &Value) -> Option<i32> {
        let id = envelope.get("id").cloned().unwrap_or(Value::Null);
        let Some(params) = envelope.get("params") else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if !self.check_binding(params) {
            self.answer_binding_mismatch(
                envelope.get("id"),
                envelope.get("params"),
            );
            return None;
        }
        let wire = match self.wire_id_of(envelope) {
            Some(wire) => wire,
            None => {
                self.answer_invalid(envelope.get("id"));
                return None;
            },
        };
        if self.table.note_inbound(wire, CallKind::Ping, None).is_err() {
            self.answer_binding_mismatch(
                envelope.get("id"),
                envelope.get("params"),
            );
            return None;
        }
        // Admitted: only now does the ping prove a live peer.
        self.observe_liveness();
        if self
            .write_control(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": ping_reply(),
            }))
            .is_err()
        {
            self.terminalize_all("ping_write_failed");
            return Some(EXIT_REFUSAL);
        }
        self.table.retire(Direction::Inbound, wire);
        None
    }

    pub(crate) fn handle_signal(&mut self, envelope: &Value) -> Option<i32> {
        let params = envelope.get("params")?;
        if !self.check_binding(params) {
            return None;
        }
        let signal = params.get("signal").and_then(Value::as_str)?;
        // The signal domain is closed: only terminate and fence-hint
        // are valid supervision signals. An unknown value is a
        // framing failure, not a liveness frame — unknown minor
        // fields are ignored, but that never extends an enumerated
        // value domain.
        if signal != "terminate" && signal != "fence-hint" {
            self.record("unknown_supervision_signal", signal);
            return None;
        }
        // Admitted: a binding-matched known signal proves a live
        // peer.
        self.observe_liveness();
        match signal {
            "terminate" => {
                self.terminalize_all("terminate");
                Some(EXIT_CLEAN)
            },
            "fence-hint" => {
                self.terminalize_all("fence_hint");
                None
            },
            _ => None,
        }
    }
}
