//! Cross-transport wedge/prime quiescence state machine.
//!
//! The three-state delivery classifier (running / unresponsive / wedged)
//! lives here so every coder transport (Tmux, Pty, and any future
//! transport) consumes the same behavioral logic. Each transport adapts
//! its native primitives to the [`WedgeProbe`] trait; the state machine
//! is generic over the trait and stays transport-agnostic.
//!
//! ## Compiled unconditionally
//!
//! This module is compiled even when the `pty` Cargo feature is OFF, because
//! the Tmux transport (always built) imports it. `pub mod quiescence;` is
//! unconditional in `src/transports/mod.rs`.
//!
//! ## Three-state classifier
//!
//! - `running` — output is flowing or settled at prompt. Returns `Ok`.
//! - `unresponsive` — prime window elapsed with no observable change and
//!   an empty inspected tail. Returns
//!   `Err(DeliveryWaitError::Timeout)`.
//! - `wedged` — pane quiesced + not prompt-ready and the inspected tail has
//!   observable non-prompt content. Returns
//!   `Err(DeliveryWaitError::Wedged)` after the counter threshold
//!   (`WEDGE_CONSECUTIVE_TICKS`) is reached.
//!
//! In addition to the three terminal classifications above, the
//! classifier recognizes a non-terminal **Busy** pre-classification:
//! when the terminal-output-write marker
//! ([`WedgeObservation::activity_generation`]) advances between two
//! consecutive observation polls, the classifier suppresses ALL
//! terminal classifications (Delivered / Timeout / Failed) and emits
//! a `delivery_target_active` diagnostic, returning
//! `QuiescenceAction::NeedsWait`. The pre-classification ordering is
//!
//! 1. Busy short-circuit (resets wedge counter, emits diagnostic,
//!    returns `NeedsWait`).
//! 2. `delivery_ready` check (terminal: returns `Done(Ok(...))`).
//! 3. Wedge-counter increment block.
//! 4. Wedge check.
//! 5. Prime timeout check.
//!
//! Busy fires on a terminal-output-write-marker advance (Tmux:
//! `#{window_activity}`; Pty: `last_change_atomic`). It does NOT fire
//! on a child process being busy with zero byte output — that case is
//! the Class B silent-thinking bug class, filed as a separate
//! follow-up proposal.
//!
//! ## Wedge-class vs empty-pane
//!
//! The mismatch helper [`mismatch_is_wedge_class`] distinguishes:
//! - **Wedge-class**: the pane has observable non-prompt content it is
//!   stuck on (e.g. a tool-approval dialog). Fires `Wedged` after the
//!   counter threshold.
//! - **Empty-pane**: the pane has NO observable content (silent/dead).
//!   Fires `Timeout` (the pane is unresponsive, not wedged).
//!
//! The signal is the inspected tail's emptiness when no structured mismatch
//! is available. When the probe supplies a [`ReadinessMismatch`], a failed
//! regex match is wedge-class while a successful regex match with a cursor
//! mismatch means the operator has pending input and remains non-terminal.

use std::{
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::runtime::{
    inscriptions::emit_delivery_diagnostic as emit_diagnostic, signals::shutdown_requested,
};
#[cfg(test)]
use crate::transports::contract::DeliveryEnvelope;
use crate::transports::contract::{DeliveryWaitError, ReadinessTimeoutReason};

/// Maximum message ids carried by one delivery-progress inscription.
pub const DIAGNOSTIC_MESSAGE_IDS_MAXIMUM: usize = 32;

/// Identity and group correlation shared by delivery-progress diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryDiagnosticContext<'a> {
    pub namespace: &'a str,
    pub target_session: &'a str,
    message_ids: Vec<String>,
    message_ids_total: usize,
}

impl<'a> DeliveryDiagnosticContext<'a> {
    #[must_use]
    pub fn new<I, S>(namespace: &'a str, target_session: &'a str, message_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut ids = Vec::new();
        let mut total = 0;
        for message_id in message_ids {
            if ids.len() < DIAGNOSTIC_MESSAGE_IDS_MAXIMUM {
                ids.push(message_id.as_ref().to_string());
            }
            total += 1;
        }
        Self {
            namespace,
            target_session,
            message_ids: ids,
            message_ids_total: total,
        }
    }

    #[must_use]
    pub fn without_messages(namespace: &'a str, target_session: &'a str) -> Self {
        Self::new(namespace, target_session, std::iter::empty::<&str>())
    }

    #[must_use]
    pub fn message_ids(&self) -> &[String] {
        self.message_ids.as_slice()
    }

    #[must_use]
    pub fn message_ids_total(&self) -> usize {
        self.message_ids_total
    }
}

/// Emits one delivery-progress diagnostic with bounded group correlation.
pub fn emit_delivery_progress(
    event: &str,
    context: &DeliveryDiagnosticContext<'_>,
    mut details: Value,
) {
    let object = details
        .as_object_mut()
        .expect("delivery diagnostic details must be an object");
    object.insert("namespace".to_string(), json!(context.namespace));
    object.insert("target_session".to_string(), json!(context.target_session));
    object.insert("message_ids".to_string(), json!(context.message_ids));
    object.insert(
        "message_ids_total".to_string(),
        json!(context.message_ids_total),
    );
    emit_diagnostic(event, &details);
}

/// Number of consecutive observation iterations showing the SAME wedge-class
/// non-prompt evaluation before wedge detection fires. Bounded by design: a
/// single quiescent tick is not enough to classify a pane as wedged because
/// agents routinely pass through transient non-prompt states (boot, tool-call
/// prep, idle-screen variations) before settling at a prompt. Counting
/// identical wedge-class mismatch signatures lets the agent transition
/// through these states without firing a false-positive wedge, while still
/// firing on a genuinely stuck state within a few quiet_window intervals.
pub const WEDGE_CONSECUTIVE_TICKS: usize = 3;

/// Default mismatch reason for the "no observable content" case. A pane
/// whose trimmed inspected tail is empty (or contains only blank lines)
/// shows this reason; it is NOT wedge-class — a silent/dead pane fires
/// `Timeout`, not `Wedged`. The classifier in [`mismatch_is_wedge_class`]
/// keeps the prefix stable for unit tests.
pub const EMPTY_PANE_MISMATCH_PREFIX: &str = "inspected pane tail was empty";

/// Transport-neutral readiness-mismatch metadata supplied by the probe
/// when the target is not prompt-ready. The state machine carries the
/// fields through to the `delivery_prompt_mismatch`, `delivery_pane_wedged`,
/// and `delivery_prime_timeout` diagnostic inscriptions so operators can
/// distinguish cursor-column mismatches from regex mismatches without
/// inspecting per-transport probe state.
///
/// Present in a [`WedgeObservation`] when `is_prompt_ready = false` and
/// the probe was able to attribute the mismatch. Absent when the probe
/// has no transport-specific readiness diagnostics to expose (the state
/// machine then falls back to deriving a generic mismatch reason from
/// the inspected tail's emptiness).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadinessMismatch {
    /// Human-readable reason for the mismatch. Examples (Tmux):
    /// `"prompt regex did not match inspected pane tail"`,
    /// `"cursor column 0 did not match required 4"`. Examples (Pty):
    /// the same shape, populated from Pty's regex + cursor analysis.
    pub reason: String,
    /// Whether the prompt regex matched the inspected tail (if the probe
    /// tracks this). `Some(true)` means the regex matched but the cursor
    /// column did not (cursor-class mismatch); `Some(false)` means the
    /// regex did not match (regex-class mismatch); `None` when the probe
    /// does not track this distinction.
    pub regex_matched: Option<bool>,
    /// The per-coder configured idle cursor column. `None` when the
    /// coder config does not constrain the cursor column.
    pub expected_cursor_column: Option<u16>,
    /// The cursor column the probe observed at the time of observation.
    /// `None` when the probe could not resolve a cursor column.
    pub observed_cursor_column: Option<u16>,
}

/// One observation snapshot returned by [`WedgeProbe::observe`].
///
/// The state machine calls `observe` twice per iteration (with
/// `quiet_window` sleep between) and compares the two snapshots to
/// determine quiescence. All fields are independent observations taken
/// from the same probe state at one instant.
///
/// A snapshot return shape (rather than separate per-field accessors)
/// keeps each transport's per-iteration cost bounded to one probe
/// roundtrip. Transports whose probes side-effect on each method call
/// (for example the tmux probe's IPC queries plus the
/// `PaneQuiescenceProbe::next_evaluation` `call_count` /
/// `abort_after_calls` counter exercised by the unit tests) would
/// otherwise do several times more work per iteration than the
/// equivalent single-snapshot read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WedgeObservation {
    /// The current inspected tail (the last `inspect_lines` rows formatted
    /// as text). Empty / whitespace-only → empty-pane (Unresponsive
    /// territory). Non-empty + not-prompt-ready → wedge-class (Wedged
    /// territory).
    pub inspected_tail: String,
    /// Whether the target is currently in a prompt-ready state. The
    /// state machine's `running` branch returns `Ok` when this is `true`.
    pub is_prompt_ready: bool,
    /// Active pane target identifier (e.g. Tmux `%0`) used by the
    /// state machine for diagnostic inscriptions. `None` when the
    /// probe does not surface a pane target (e.g. Pty, which has no
    /// tmux-style pane id); the state machine omits the field from
    /// diagnostics in that case.
    pub pane_target: Option<String>,
    /// Readiness-mismatch metadata. Present when `is_prompt_ready =
    /// false` and the probe has transport-specific diagnostics to
    /// expose (regex vs cursor, expected vs observed cursor columns).
    /// The state machine uses `mismatch.reason` for the
    /// wedge/prime-timeout `reason` payload; falls back to deriving
    /// a generic reason from the inspected tail when this is `None`.
    pub mismatch: Option<ReadinessMismatch>,
    /// Terminal-output-write marker captured at observation time.
    /// Populated by each transport's `observe()` from a transport-
    /// native primitive: Tmux uses `#{window_activity}` parsed as a
    /// `u64` epoch-seconds value (falling back to `0` when the
    /// format is unavailable on the running tmux version); Pty
    /// uses `last_change_atomic.load(Ordering::Acquire)` from
    /// `PtyShared`. A monotonic counter / generation; an advance
    /// between two consecutive observations signals that bytes were
    /// written to the target during the `quiet_window`, which the
    /// classifier uses to suppress all terminal classifications
    /// (Busy pre-classification — see module-level docs). This is a
    /// terminal-output-write marker, NOT a process-busy marker: a
    /// target whose agent is in silent thinking with zero byte output
    /// produces a constant value here and the Busy pre-classification
    /// does NOT fire (that's the Class B silent-thinking case,
    /// filed as a follow-up).
    pub activity_generation: u64,
}

/// Probe abstraction for the wedge/prime state machine.
///
/// Each transport implements this trait with its native primitives. The
/// state machine calls [`observe`](WedgeProbe::observe) twice per
/// iteration (with `quiet_window` sleep between) and compares the
/// returned snapshots to determine quiescence.
pub trait WedgeProbe {
    /// Capture the probe's current state as a single observation
    /// snapshot. Called twice per quiescence iteration. Implementations
    /// are expected to read any underlying IPC / state once and return a
    /// consistent snapshot.
    fn observe(&mut self) -> Result<WedgeObservation, String>;

    /// Block until the target shows a change (the next observation would
    /// differ from the previous one) or the supplied `deadline` elapses.
    /// Returns `Ok(())` on observed change; `Err(DeliveryWaitError::Timeout)`
    /// on deadline elapsed with no change; `Err(DeliveryWaitError::Failed)`
    /// on probe errors. The state machine passes a deadline derived from
    /// the per-coder `prime_timeout_ms` so the probe honors the same prime
    /// window the loop tracks.
    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError>;
}

/// Signature of a non-ready observation used to dedup the
/// `delivery_prompt_mismatch` diagnostic emitted from the quiescence wait.
/// When the pane is stuck on the same non-matching state, repeated identical
/// observations across poll ticks collapse to a single inscription. The
/// dialog is still treated as non-quiescent and delivery still blocks
/// until the state clears.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MismatchSignature {
    mismatch_reason: Option<String>,
    inspected_block: Option<String>,
    regex_matched: Option<bool>,
    expected_cursor_column: Option<u16>,
    observed_cursor_column: Option<u16>,
}

impl MismatchSignature {
    fn from_observation(snapshot: &WedgeObservation) -> Self {
        let (reason, regex_matched, expected, observed) = match &snapshot.mismatch {
            Some(m) => (
                Some(m.reason.clone()),
                m.regex_matched,
                m.expected_cursor_column,
                m.observed_cursor_column,
            ),
            None => (None, None, None, None),
        };
        let inspected_block = if snapshot.inspected_tail.trim().is_empty() {
            None
        } else {
            Some(snapshot.inspected_tail.clone())
        };
        Self {
            mismatch_reason: reason,
            inspected_block,
            regex_matched,
            expected_cursor_column: expected,
            observed_cursor_column: observed,
        }
    }
}

/// Returns whether a fresh `delivery_prompt_mismatch` diagnostic should
/// be emitted. The first call after entering the wait, and every call
/// whose signature differs from the last emitted one, returns `true` and
/// updates `last`. Repeated identical signatures return `false`.
fn should_emit_prompt_mismatch(
    last: &mut Option<MismatchSignature>,
    current: &MismatchSignature,
) -> bool {
    if last.as_ref() == Some(current) {
        false
    } else {
        *last = Some(current.clone());
        true
    }
}

/// Returns whether a mismatch reason indicates a wedge-class state (the
/// pane has settled at some specific non-prompt content) versus a
/// dead-pane state (no observable content).
///
/// A structured cursor mismatch with `regex_matched = Some(true)` means
/// the prompt frame is healthy but the operator has pending input. It must
/// remain pending rather than being classified as a wedged agent. Regex
/// mismatches with observable content remain wedge-class. Empty-pane
/// mismatches are Unresponsive territory instead.
pub fn mismatch_is_wedge_class(snapshot: &WedgeObservation) -> bool {
    if snapshot
        .mismatch
        .as_ref()
        .is_some_and(|mismatch| mismatch.regex_matched == Some(true))
    {
        return false;
    }
    let mismatch_reason = resolve_mismatch_reason(snapshot);
    mismatch_reason
        .as_deref()
        .is_some_and(|reason| !reason.starts_with(EMPTY_PANE_MISMATCH_PREFIX))
}

/// Returns the mismatch reason to use for the wedge/prime-timeout
/// `reason` payload. When the observation's [`WedgeObservation::mismatch`]
/// field is present, returns a clone of that reason (preserving the
/// probe-supplied distinction between regex vs cursor mismatches).
/// Otherwise, derives a generic reason from the inspected tail's
/// emptiness for backward compatibility with probes that do not supply
/// structured readiness metadata.
fn resolve_mismatch_reason(snapshot: &WedgeObservation) -> Option<String> {
    if let Some(mismatch) = &snapshot.mismatch {
        return Some(mismatch.reason.clone());
    }
    if snapshot.is_prompt_ready {
        return None;
    }
    if snapshot.inspected_tail.trim().is_empty() {
        Some(format!(
            "{EMPTY_PANE_MISMATCH_PREFIX} (no observable content)"
        ))
    } else {
        Some("prompt regex did not match inspected pane tail".to_string())
    }
}

/// Per-delivery state carried across [`quiescence_classify_step`]
/// invocations. Tracks the consecutive-quiescent-mismatch counter and
/// the last emitted mismatch signature for diagnostic deduplication.
///
/// Public so the Pty worker (which drives the state machine one step
/// at a time between channel-service iterations) can construct a
/// fresh state per delivery and pass `&mut` to each step.
#[derive(Default)]
pub struct QuiescenceState {
    last_mismatch_signature: Option<MismatchSignature>,
    consecutive_quiescent_mismatches: usize,
}

impl QuiescenceState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current consecutive-quiescent-mismatch counter
    /// value the wedge classifier uses to fire `Wedged` after the
    /// `WEDGE_CONSECUTIVE_TICKS` threshold. Exposed for tests and
    /// for callers that need to observe classifier-side state
    /// without taking a `&mut` borrow (e.g., diagnostic readouts).
    #[must_use]
    pub fn consecutive_quiescent_mismatches(&self) -> usize {
        self.consecutive_quiescent_mismatches
    }
}

/// The bounds one flush group's quiescence wait is subject to.
///
/// Grouped rather than passed as loose arguments because they are one cohesive
/// unit: every field is fixed at group formation from the head envelope, and
/// none changes for the life of the group. Deriving both deadlines from a
/// single `started_at` is what implements the spec's requirement that the
/// readiness bound share the prime window's anchor, and that neither resets
/// when coalesce-during-wait absorbs a later envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuiescenceBounds {
    /// Quiet period slept between the two observations of one iteration.
    pub quiet_window: Duration,
    /// The instant the group's wait began; the anchor for both deadlines.
    pub started_at: Instant,
    /// Opt-in prime-window deadline. `None` issues no prime-window verdict.
    /// Its absence does NOT make the wait unbounded — `readiness_deadline`
    /// applies regardless, where the transport defines one.
    pub prime_deadline: Option<Instant>,
    /// The configured prime window, carried for diagnostics only.
    pub prime_timeout_ms: Option<u64>,
    /// Unconditional bound on the entire wait, for transports whose readiness
    /// contract defines one. `None` for a transport that has not defined one;
    /// that absence SHALL NOT be read as the transport being bounded by some
    /// other means. See `agentmux:issues/relay/61`.
    pub readiness_deadline: Option<Instant>,
}

impl QuiescenceBounds {
    /// Builds the bounds for one flush group from its head envelope's hints.
    #[must_use]
    pub fn new(
        quiet_window: Duration,
        started_at: Instant,
        prime_timeout_ms: Option<u64>,
        readiness_timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            quiet_window,
            started_at,
            prime_deadline: prime_timeout_ms.map(|ms| started_at + Duration::from_millis(ms)),
            prime_timeout_ms,
            readiness_deadline: readiness_timeout_ms
                .map(|ms| started_at + Duration::from_millis(ms)),
        }
    }

    /// Builds the bounds for a flush group from the group's **head** envelope,
    /// ignoring every later one.
    ///
    /// This is the operation the anchoring rule lives in. A transport that
    /// indexes the head inline states the rule nowhere, so nothing can test it
    /// and coalesce-during-wait can silently start honoring a later envelope's
    /// hints. Taking the whole group and discarding the tail makes "the head
    /// owns the bounds" the function's observable behavior: pass a group whose
    /// later envelopes carry different hints and the result is unchanged.
    ///
    /// Returns `None` for an empty group, which has no head to anchor to.
    ///
    /// Crate-private: its only caller is the Tmux transport, and nothing
    /// outside this crate constructs a flush group. Its coverage is the single
    /// inline test at the bottom of this module rather than one in
    /// `tests/unit`, which would require widening this to `pub` — publishing an
    /// internal grouping rule as API to observe it.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_group<'a>(
        envelopes: impl IntoIterator<Item = &'a DeliveryEnvelope>,
        started_at: Instant,
    ) -> Option<Self> {
        let head = envelopes.into_iter().next()?;
        Some(Self::new(
            head.quiet_window,
            started_at,
            head.prime_timeout_ms,
            head.readiness_timeout_ms,
        ))
    }

    /// Reports whether the readiness bound has elapsed as of `now`.
    #[must_use]
    fn readiness_elapsed(&self, now: Instant) -> bool {
        self.readiness_deadline
            .is_some_and(|deadline| now >= deadline)
    }

    /// Caps a proposed wait deadline at the readiness bound.
    ///
    /// Every `NeedsWait` deadline passes through here, so no wait the
    /// classifier schedules can outlive the bound. A group carrying no bound
    /// is returned unchanged, which is what keeps Pty's behavior identical.
    #[must_use]
    fn cap(&self, deadline: Instant) -> Instant {
        match self.readiness_deadline {
            Some(bound) => deadline.min(bound),
            None => deadline,
        }
    }

    /// The quiet period to sleep between an iteration's two observations,
    /// shortened so the pair cannot run past the readiness bound.
    ///
    /// Capping the `NeedsWait` deadlines alone is not enough to make the bound
    /// hard. The wrapper wakes at the deadline, then re-enters the classifier,
    /// which observes, sleeps, observes, and only then evaluates the bound — so
    /// an uncapped sleep reports expiry a full quiet window late and opens a
    /// post-deadline interval in which a target that was not ready at the
    /// deadline can still deliver. The window is the wait's own sampling
    /// interval, and an operator may set it per send, so the overshoot is not
    /// negligible by construction.
    ///
    /// The final iteration's pair is therefore sampled over a shorter interval
    /// than the ones before it. That can only make the expiry's *reason* less
    /// specific — an activity advance is less likely to be caught in a shorter
    /// window — and the reason is diagnostic. The outcome is unaffected: every
    /// arm resolves the same way, and a prompt-ready second observation still
    /// outranks the elapsed bound and delivers.
    #[must_use]
    fn sleep_window(&self, now: Instant) -> Duration {
        match self.readiness_deadline {
            Some(bound) => self.quiet_window.min(bound.saturating_duration_since(now)),
            None => self.quiet_window,
        }
    }
}

/// Outcome of a single [`quiescence_classify_step`] call. The state
/// machine either resolves (Done) or needs the caller to wait for a
/// change before the next step.
#[derive(Clone, Debug, PartialEq)]
pub enum QuiescenceAction {
    /// The wait has resolved. The caller should propagate the result.
    Done(Result<String, DeliveryWaitError>),
    /// The state machine needs to wait for a change before continuing.
    /// `deadline` is the prime-timeout deadline the wait must honor
    /// (capped at `prime_deadline` when set; otherwise a 1-year
    /// unbounded deadline).
    NeedsWait(Instant),
}

/// One step of the three-state delivery classifier (running /
/// unresponsive / wedged) over a [`WedgeProbe`].
///
/// Splits the body of the previous `wait_for_quiescent_three_state`
/// loop into a steppable form: each call performs one
/// observe-sleep-observe-classify cycle. The caller drives the
/// `wait_for_change` step between calls. This lets the Pty worker
/// thread service other channels (PTY byte drain, snapshot requests,
/// write_rx absorption) BETWEEN steps, instead of blocking the worker
/// inside a long wait.
///
/// See [`wait_for_quiescent_three_state`] for the timing contract
/// (both deadlines anchored to "delivery-task perspective", neither reset
/// across coalesce iterations).
///
/// # Readiness bound
///
/// [`QuiescenceBounds::readiness_deadline`], where the transport defines one,
/// is the **unconditional termination guarantee** for the wait: it applies
/// whatever the target shows and whether or not a prime timeout is configured,
/// and no signal defers, extends, or suspends it. It is not the only terminal
/// path — an opted-in prime timeout, shutdown, and a positively observed probe
/// failure each remain terminal — but it is the only one guaranteed to arrive.
///
/// It is deliberately **not** a branch in the ordering below. It is a
/// precondition on the iteration's result: no branch may return `NeedsWait`
/// once it has elapsed, and every `NeedsWait` deadline is capped at it. Writing
/// it as a positional early return would report the readiness outcome in an
/// iteration where a higher-precedence outcome was available. Outcome
/// precedence, highest first: delivery, then the prime timeout, then the
/// readiness bound.
///
/// # What absence of change does not mean
///
/// A target that is not ready is a target that is not ready *now*. The reason
/// is not knowable from the inspected tail: a permission dialog awaiting an
/// operator, a compose box holding typed input, a coder producing no terminal
/// output while working, and a hung process all present as a settled non-prompt
/// frame. Only a positively observed terminal event — process death, a closed
/// connection, a protocol error — is sound evidence of failure. Tmux exposes
/// `pane_dead`, but only under `remain-on-exit`, which this system does not set;
/// without it a dead process destroys the pane and the resulting probe failure
/// already resolves the wait. That path is left unbuilt deliberately.
///
/// The `wedged` classification draws exactly the inference this warns against.
/// Pty is its only remaining caller, retained because it is Pty's sole terminal
/// path until `agentmux:issues/relay/61` supplies a Pty readiness bound.
pub fn quiescence_classify_step<W: WedgeProbe>(
    probe: &mut W,
    state: &mut QuiescenceState,
    diagnostics: &DeliveryDiagnosticContext<'_>,
    bounds: &QuiescenceBounds,
    wedge_detection: bool,
) -> QuiescenceAction {
    if shutdown_requested() {
        return QuiescenceAction::Done(Err(DeliveryWaitError::Shutdown));
    }

    // --- Observation 1 (before sleep) ---------------------------------
    let snapshot_before = match probe.observe() {
        Ok(s) => s,
        Err(reason) => return QuiescenceAction::Done(Err(DeliveryWaitError::Failed { reason })),
    };

    thread::sleep(bounds.sleep_window(Instant::now()));
    if shutdown_requested() {
        return QuiescenceAction::Done(Err(DeliveryWaitError::Shutdown));
    }

    // --- Observation 2 (after sleep) ----------------------------------
    let snapshot_after = match probe.observe() {
        Ok(s) => s,
        Err(reason) => return QuiescenceAction::Done(Err(DeliveryWaitError::Failed { reason })),
    };

    // Sampled once so every bound in this iteration is evaluated against the
    // same instant. Re-reading the clock per branch would let one bound
    // observe an elapse the branch above it did not, which is how a
    // precedence rule silently stops holding.
    let now = Instant::now();
    let readiness_elapsed = bounds.readiness_elapsed(now);

    // Quiescence: both observations agree across all signals
    // (including pane target and mismatch metadata and the
    // activity_generation terminal-output-write marker).
    let quiescent = snapshot_before == snapshot_after;

    // --- Busy short-circuit. ------------------------------------------
    // When the terminal-output-write marker advanced between two
    // consecutive observations, the target is mid-generation: bytes
    // are flowing but the snapshot might not yet reflect them
    // (Class A: partial escape-sequence redraws, scroll-during-
    // capture, cursor-blanking sequences, etc.). Suppress ALL
    // terminal classifications (Delivered / Timeout / Failed) and
    // emit `delivery_target_active` so operators can see when the
    // short-circuit fired. The wedge counter resets because we
    // return early before the increment block — this is an
    // implicit guard from the early `return`. The diagnostic dedups
    // by generation: an iteration whose activity_generation did not
    // advance does not enter this branch.
    //
    // SCOPE: this fires only on terminal-output-write activity
    // advance. It does NOT fire on a child process being busy with
    // zero byte output (Class B silent-thinking); that case is filed
    // as a follow-up proposal with a process-level aliveness signal.
    if snapshot_before.activity_generation != snapshot_after.activity_generation {
        state.consecutive_quiescent_mismatches = 0;
        emit_delivery_progress(
            "delivery_target_active",
            diagnostics,
            json!({
                "pane_target": snapshot_after.pane_target,
                "activity_delta": snapshot_after
                    .activity_generation
                    .saturating_sub(snapshot_before.activity_generation),
            }),
        );
        // Busy suppression is bounded, not indefinite. The
        // terminal-output-write signal is itself unbounded — a target may
        // emit bytes forever without ever becoming ready — so suppression
        // keyed to it is an unbounded wait unless something outranks it.
        // This is the defect the readiness bound exists to close, and the
        // reason it must be checked here rather than only on the settled
        // path below.
        if readiness_elapsed {
            return readiness_expiry(diagnostics, bounds, &snapshot_before, &snapshot_after, now);
        }
        // Return `NeedsWait` with a SHORT deadline (`now + quiet_window`)
        // rather than the prime deadline. The prime deadline can be
        // unbounded when `prime_timeout_ms = None`; production
        // `wait_for_change` (Tmux: polls `#{window_activity}` / snapshot;
        // Pty: polls `last_change_atomic`) blocks until a transport
        // change OR the deadline elapses. After a Busy iteration the
        // activity has just advanced during the observe-sleep-observe
        // pair; if it then settles and there is no subsequent change
        // to wake the loop, the wrapper would otherwise block on an
        // unbounded deadline and `delivery_ready` would never fire on
        // the next iteration. Bounding the deadline at
        // `quiet_window` keeps re-classification cadence tightly
        // tied to the polling interval — production Tmux wakes up
        // at most every `quiet_window` after the activity has settled,
        // enough to deliver promptly without artificial signals.
        return QuiescenceAction::NeedsWait(bounds.cap(now + bounds.quiet_window));
    }

    // --- `running` — pane is ready. -----------------------------------
    // After the Busy short-circuit, so a post-sleep snapshot that
    // matches the prompt regex while activity was advancing during
    // the same quiet_window fires Busy above (returning NeedsWait)
    // rather than Delivered here.
    //
    // Delivery outranks an elapsed readiness bound, which is why this
    // branch does not consult `readiness_elapsed`. Reaching readiness late
    // is the outcome the wait existed to obtain; the bound exists to stop
    // waiting forever, not to refuse a success already in hand.
    if snapshot_after.is_prompt_ready {
        emit_delivery_progress(
            "delivery_ready",
            diagnostics,
            json!({
                "pane_target": snapshot_after.pane_target,
            }),
        );
        return QuiescenceAction::Done(Ok(snapshot_after.pane_target.unwrap_or_default()));
    }

    let mismatch_reason = resolve_mismatch_reason(&snapshot_after);
    let wedge_class = mismatch_is_wedge_class(&snapshot_after);

    // Track consecutive identical wedge-class non-prompt evaluations.
    // The counter increments ONLY for wedge-class mismatches; empty-pane
    // mismatches do not increment (they are Unresponsive territory,
    // not Wedged). The counter also resets whenever the wedge-class
    // signature changes (the pane transitioned through a different
    // stuck state), so transient non-prompt states (e.g. boot output
    // before the prompt appears) do not accumulate wedge ticks.
    if !snapshot_after.is_prompt_ready && quiescent && wedge_class {
        let signature = MismatchSignature::from_observation(&snapshot_after);
        match state.last_mismatch_signature.as_ref() {
            Some(previous) if previous == &signature => {
                state.consecutive_quiescent_mismatches =
                    state.consecutive_quiescent_mismatches.saturating_add(1);
            }
            _ => {
                state.consecutive_quiescent_mismatches = 1;
            }
        }
        state.last_mismatch_signature = Some(signature);
    } else {
        state.consecutive_quiescent_mismatches = 0;
    }

    // Wedge check: fires immediately on prime-timeout elapse when
    // the pane is showing wedge-class content, OR after the counter
    // threshold for any wedge-class mismatch even if the prime
    // window has not elapsed.
    if wedge_detection && quiescent && !snapshot_after.is_prompt_ready && wedge_class {
        let counter_fires = state.consecutive_quiescent_mismatches >= WEDGE_CONSECUTIVE_TICKS;
        let prime_elapsed = bounds
            .prime_deadline
            .is_some_and(|deadline| now >= deadline);
        if counter_fires || prime_elapsed {
            emit_delivery_progress(
                "delivery_pane_wedged",
                diagnostics,
                json!({
                    "pane_target": snapshot_after.pane_target,
                    "mismatch_reason": mismatch_reason,
                    "consecutive_quiescent_ticks": state.consecutive_quiescent_mismatches,
                    "fired_via_prime_timeout": prime_elapsed && !counter_fires,
                }),
            );
            return QuiescenceAction::Done(Err(DeliveryWaitError::Wedged {
                reason: mismatch_reason.unwrap_or_else(|| {
                    "pane wedged at non-prompt state with no recorded mismatch reason".to_string()
                }),
            }));
        }
    }

    // Prime timeout check. Fires `Timeout` when the prime window has elapsed,
    // except on a settled wedge-class frame *in a group that carries a
    // readiness bound*.
    //
    // The exclusion used to be implicit: wedge-class content returned from the
    // wedge branch above before reaching here. With wedge detection off for
    // Tmux that branch no longer intercepts, so the exclusion is stated
    // directly. Without it, removing wedge detection would have silently
    // widened the prime timeout's scope from "the target never produced
    // observable output" to "the target is not ready" — which is the same
    // inference from absence that the wedge classifier was removed for making,
    // just wearing the prime timeout's reason code. A settled permission
    // dialog is a target that answered.
    //
    // The bound is what earns the exclusion, which is why it is part of the
    // condition rather than assumed. Prime may stop adjudicating a settled
    // frame only where something else is guaranteed to end the wait. A group
    // with no readiness bound has nothing to hand the duty to: for Pty with
    // `wedge-detection = false` — no wedge branch, no bound — the prime
    // timeout is the only terminal path a settled frame can reach, and
    // excluding it there strands the wait forever. This is the same reasoning
    // the retired `Tmux prime timeout bounds post-quiescence wait when wedge is
    // disabled` scenario encoded; it stopped applying to Tmux because Tmux
    // gained a bound, not because it stopped being true.
    //
    // Checked before the readiness bound: when both have elapsed in the same
    // iteration the prime timeout wins, as the more specific diagnosis (no
    // observable output at all) and the one an operator opted into.
    if let Some(deadline) = bounds.prime_deadline
        && now >= deadline
        && !(quiescent && wedge_class && bounds.readiness_deadline.is_some())
    {
        let timeout_ms = bounds.prime_timeout_ms.unwrap_or(0);
        let timeout = Duration::from_millis(timeout_ms);
        let elapsed_ms = now.saturating_duration_since(bounds.started_at).as_millis();
        let readiness_mismatch = !snapshot_after.is_prompt_ready;
        emit_delivery_progress(
            "delivery_prime_timeout",
            diagnostics,
            json!({
                "pane_target": snapshot_after.pane_target,
                "timeout_ms": timeout_ms,
                "prime_wait_elapsed_ms": elapsed_ms,
                "readiness_mismatch": readiness_mismatch,
                "mismatch_reason": mismatch_reason,
            }),
        );
        return QuiescenceAction::Done(Err(DeliveryWaitError::Timeout {
            timeout,
            readiness_mismatch,
            mismatch_reason,
        }));
    }

    // Readiness bound: the lowest-precedence outcome, reached only when no
    // delivery, wedge, or prime-timeout outcome was available above.
    if readiness_elapsed {
        return readiness_expiry(diagnostics, bounds, &snapshot_before, &snapshot_after, now);
    }

    // Non-quiesced or wedge not yet at threshold: emit the dedup'd
    // prompt-mismatch diagnostic if applicable.
    if !snapshot_after.is_prompt_ready {
        let signature = MismatchSignature::from_observation(&snapshot_after);
        if should_emit_prompt_mismatch(&mut state.last_mismatch_signature, &signature) {
            emit_delivery_progress(
                "delivery_prompt_mismatch",
                diagnostics,
                json!({
                    "pane_target": snapshot_after.pane_target,
                    "mismatch_reason": signature.mismatch_reason,
                    "regex_matched": signature.regex_matched,
                    "inspected_block": signature.inspected_block,
                    "expected_cursor_column": signature.expected_cursor_column,
                    "observed_cursor_column": signature.observed_cursor_column,
                }),
            );
        }
    }

    let deadline = if wedge_detection && quiescent && wedge_class {
        let recheck = now + bounds.quiet_window;
        bounds
            .prime_deadline
            .map_or(recheck, |prime| prime.min(recheck))
    } else {
        bounds.prime_deadline.unwrap_or_else(unbounded_deadline)
    };
    // Capping here is what removes the unbounded fall-through for a group
    // that carries a bound, while leaving a group without one untouched.
    QuiescenceAction::NeedsWait(bounds.cap(deadline))
}

/// Resolves a flush group whose readiness bound has elapsed.
///
/// Every arm is the same outcome — `SendOutcome::Timeout` — and the reason
/// only tells an operator which observation the wait ended on.
fn readiness_expiry(
    diagnostics: &DeliveryDiagnosticContext<'_>,
    bounds: &QuiescenceBounds,
    snapshot_before: &WedgeObservation,
    snapshot_after: &WedgeObservation,
    now: Instant,
) -> QuiescenceAction {
    let reason_code = classify_readiness_timeout_reason(snapshot_before, snapshot_after);
    let mismatch_reason = resolve_mismatch_reason(snapshot_after);
    let elapsed = now.saturating_duration_since(bounds.started_at);
    emit_delivery_progress(
        "delivery_readiness_timeout",
        diagnostics,
        json!({
            "pane_target": snapshot_after.pane_target,
            "reason_code": reason_code.code(),
            "mismatch_reason": mismatch_reason,
            "readiness_wait_elapsed_ms": elapsed.as_millis(),
        }),
    );
    QuiescenceAction::Done(Err(DeliveryWaitError::ReadinessTimeout {
        reason_code,
        elapsed,
        mismatch_reason,
    }))
}

/// Classifies an observation pair into the reason a readiness bound expired.
///
/// Precedence, highest first: activity advancing, an empty inspected tail, a
/// cursor mismatch on a healthy prompt frame, then frame absence. Activity
/// ranks first because it is the only signal describing the *pair* rather than
/// the final snapshot — by the time the bound elapses the last snapshot may
/// look settled even though the target never was.
#[must_use]
pub fn classify_readiness_timeout_reason(
    snapshot_before: &WedgeObservation,
    snapshot_after: &WedgeObservation,
) -> ReadinessTimeoutReason {
    if snapshot_before.activity_generation != snapshot_after.activity_generation {
        return ReadinessTimeoutReason::TargetNeverSettled;
    }
    if snapshot_after.inspected_tail.trim().is_empty() {
        return ReadinessTimeoutReason::TargetUnresponsive;
    }
    if snapshot_after
        .mismatch
        .as_ref()
        .is_some_and(|mismatch| mismatch.regex_matched == Some(true))
    {
        return ReadinessTimeoutReason::PendingOperatorInput;
    }
    ReadinessTimeoutReason::TargetNotReady
}

/// Returns a one-year deadline used when the caller passes `None` for
/// the prime timeout (i.e. unbounded wait). Bounded so the
/// `wait_for_change` polling loop terminates on a sane horizon even
/// if the probe never reports a change.
///
/// Only reachable for a flush group carrying no readiness bound;
/// [`QuiescenceBounds::cap`] shortens it to the bound otherwise.
fn unbounded_deadline() -> Instant {
    Instant::now() + Duration::from_secs(60 * 60 * 24 * 365)
}

/// Drives the three-state delivery classifier (running / unresponsive /
/// wedged) over a [`WedgeProbe`].
///
/// Thin wrapper over [`quiescence_classify_step`]: performs one
/// observe-sleep-observe-classify cycle, then calls
/// `probe.wait_for_change(deadline)` before the next cycle. The
/// generic cross-transport callers (Tmux, Pty cross-thread probe)
/// can use this entry point directly; transports that need to service
/// other channels between steps (the Pty worker, see
/// `pty::delivery`) drive [`quiescence_classify_step`] directly.
///
/// `bounds` is fixed for the whole wait — built by the caller at group
/// formation and NOT rebuilt across iterations, because both deadlines are
/// anchored to "delivery-task perspective (when flush begins, not enqueue
/// time)" and neither may be extended by a coalesced envelope.
///
/// Precedence: a pane that has settled into a non-prompt state
/// (quiescent + not-ready) is wedge
/// territory — prime_timeout MUST NOT fire in this case. Prime timeout
/// only fires while the pane is still active (changing between
/// observation ticks) or when wedge detection is disabled and the pane
/// never settles. The readiness bound overrides all of that: it resolves
/// the group whatever the pane is doing.
///
/// Returns `Ok(pane_target)` on the running branch (the pane is
/// prompt-ready); the pane target is the
/// value the probe reported in the successful observation, or an empty
/// string when the probe did not surface one. Returns
/// `Err(DeliveryWaitError::...)` on the unresponsive / wedged / readiness /
/// failed / shutdown branches.
pub fn wait_for_quiescent_three_state<W: WedgeProbe>(
    probe: &mut W,
    diagnostics: &DeliveryDiagnosticContext<'_>,
    bounds: &QuiescenceBounds,
    wedge_detection: bool,
) -> Result<String, DeliveryWaitError> {
    let mut state = QuiescenceState::new();
    loop {
        match quiescence_classify_step(probe, &mut state, diagnostics, bounds, wedge_detection) {
            QuiescenceAction::Done(result) => return result,
            QuiescenceAction::NeedsWait(deadline) => {
                // Block until the probe reports a change or the prime
                // deadline elapses. This advances the probe's state
                // for the NEXT iteration (Tmux: wait_for_change polls
                // tmux IPC for activity; Pty: bytes arriving on the
                // master). When the probe has nothing to advance
                // (test probe sequence exhausted, or no bytes
                // arriving), wait_for_change blocks until the
                // deadline.
                match probe.wait_for_change(deadline) {
                    Ok(()) => {}
                    Err(DeliveryWaitError::Timeout { .. }) => {
                        // No change observed before the prime deadline.
                        // The prime-timeout branch above fires on the
                        // next iteration if the deadline has elapsed.
                    }
                    Err(other) => return Err(other),
                }
            }
        }
    }
}

/// Inline because [`QuiescenceBounds::from_group`] is crate-private: its only
/// caller is the Tmux transport, and nothing outside this crate constructs a
/// flush group. Covering it from `tests/unit` would mean widening it to `pub`,
/// publishing an internal grouping rule as API to observe it.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::AddressIdentity;
    use crate::transports::contract::DeliveryMessage;

    /// Builds an envelope carrying the quiescence hints a flush group anchors on.
    fn hinted_envelope(
        message_id: &str,
        quiet_window: Duration,
        prime_timeout_ms: Option<u64>,
        readiness_timeout_ms: Option<u64>,
    ) -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: message_id.to_string(),
            message: DeliveryMessage {
                body: "body".to_string(),
                created_at: "2026-08-01T00:00:00Z".to_string(),
                namespace: "test-namespace".to_string(),
                sender: AddressIdentity {
                    session_name: "sender".to_string(),
                    display_name: None,
                },
                target: AddressIdentity {
                    session_name: "target".to_string(),
                    display_name: None,
                },
                cc: Vec::new(),
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            quiet_window,
            prime_timeout_ms,
            readiness_timeout_ms,
            is_receipt: false,
        }
    }

    /// Task 4.12, production shape — the head envelope owns the group's bounds
    /// and a later one absorbed by coalesce-during-wait cannot shift them.
    ///
    /// The anchoring rule used to live as a bare `group[0]` index at the Tmux
    /// call site, where nothing could observe it: every later envelope's hints
    /// were discarded by an expression, not by a rule. Deriving the bounds from
    /// the whole group and discarding the tail makes head-ownership the
    /// function's behavior, so this test can state it. Every later hint here
    /// differs from the head's in both directions, so a `last()`, a `min`, or a
    /// `max` would each be caught.
    #[test]
    fn a_groups_bounds_come_from_its_head_envelope_only() {
        let head_quiet_window = Duration::from_millis(1);
        let started_at = Instant::now();
        let group = [
            hinted_envelope("head", head_quiet_window, Some(1_000), Some(5_000)),
            hinted_envelope("later-shorter", Duration::from_secs(9), Some(10), Some(30)),
            hinted_envelope(
                "later-longer",
                Duration::from_secs(99),
                Some(900_000),
                Some(3_600_000),
            ),
        ];
        let bounds = QuiescenceBounds::from_group(group.iter(), started_at)
            .expect("a non-empty group has a head");

        assert_eq!(bounds.quiet_window, head_quiet_window);
        assert_eq!(bounds.prime_timeout_ms, Some(1_000));
        assert_eq!(
            bounds.prime_deadline,
            Some(started_at + Duration::from_millis(1_000)),
        );
        assert_eq!(
            bounds.readiness_deadline,
            Some(started_at + Duration::from_millis(5_000)),
        );

        // A group with nothing in it has no head to anchor to, which is how the
        // production call site subsumes its own emptiness check.
        assert!(QuiescenceBounds::from_group(&[], started_at).is_none());
    }
}
