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
//! - `unresponsive` — prime window elapsed with no observable change AND
//!   no operator interaction AND an empty inspected tail. Returns
//!   `Err(DeliveryWaitError::Timeout)`.
//! - `wedged` — pane quiesced + not prompt-ready + no operator interaction
//!   AND the inspected tail has observable non-prompt content. Returns
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

use serde_json::json;

use crate::runtime::{inscriptions::emit_delivery_diagnostic, signals::shutdown_requested};
use crate::transports::contract::DeliveryWaitError;

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
/// (prime_deadline anchored to "delivery-task perspective", operator
/// interaction indefinitely suppressing both classifiers, etc.).
#[allow(clippy::too_many_arguments)]
pub fn quiescence_classify_step<W: WedgeProbe>(
    probe: &mut W,
    state: &mut QuiescenceState,
    target_session: &str,
    quiet_window: Duration,
    prime_deadline: Option<Instant>,
    prime_started_at: Instant,
    prime_timeout_ms: Option<u64>,
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

    thread::sleep(quiet_window);
    if shutdown_requested() {
        return QuiescenceAction::Done(Err(DeliveryWaitError::Shutdown));
    }

    // --- Observation 2 (after sleep) ----------------------------------
    let snapshot_after = match probe.observe() {
        Ok(s) => s,
        Err(reason) => return QuiescenceAction::Done(Err(DeliveryWaitError::Failed { reason })),
    };

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
        emit_delivery_diagnostic(
            "delivery_target_active",
            &json!({
                "target_session": target_session,
                "pane_target": snapshot_after.pane_target,
                "activity_delta": snapshot_after
                    .activity_generation
                    .saturating_sub(snapshot_before.activity_generation),
            }),
        );
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
        return QuiescenceAction::NeedsWait(Instant::now() + quiet_window);
    }

    // --- `running` — pane is ready. -----------------------------------
    // After the Busy short-circuit, so a post-sleep snapshot that
    // matches the prompt regex while activity was advancing during
    // the same quiet_window fires Busy above (returning NeedsWait)
    // rather than Delivered here.
    if snapshot_after.is_prompt_ready {
        emit_delivery_diagnostic(
            "delivery_ready",
            &json!({
                "target_session": target_session,
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
        let prime_elapsed = prime_deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if counter_fires || prime_elapsed {
            emit_delivery_diagnostic(
                "delivery_pane_wedged",
                &json!({
                    "target_session": target_session,
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

    // Prime timeout check: hard bound on the total wait. Fires
    // `Timeout` when the prime window has elapsed AND the pane is
    // NOT showing wedge-class content. Wedge-class content takes
    // the wedge branch above; the pane is stuck, not unresponsive.
    if let Some(deadline) = prime_deadline
        && Instant::now() >= deadline
    {
        let timeout_ms = prime_timeout_ms.unwrap_or(0);
        let timeout = Duration::from_millis(timeout_ms);
        let elapsed_ms = Instant::now()
            .saturating_duration_since(prime_started_at)
            .as_millis();
        let readiness_mismatch = !snapshot_after.is_prompt_ready;
        emit_delivery_diagnostic(
            "delivery_prime_timeout",
            &json!({
                "target_session": target_session,
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

    // Non-quiesced or wedge not yet at threshold: emit the dedup'd
    // prompt-mismatch diagnostic if applicable.
    if !snapshot_after.is_prompt_ready {
        let signature = MismatchSignature::from_observation(&snapshot_after);
        if should_emit_prompt_mismatch(&mut state.last_mismatch_signature, &signature) {
            emit_delivery_diagnostic(
                "delivery_prompt_mismatch",
                &json!({
                    "target_session": target_session,
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
        let recheck = Instant::now() + quiet_window;
        prime_deadline.map_or(recheck, |prime| prime.min(recheck))
    } else {
        prime_deadline.unwrap_or_else(unbounded_deadline)
    };
    QuiescenceAction::NeedsWait(deadline)
}

/// Returns a one-year deadline used when the caller passes `None` for
/// the prime timeout (i.e. unbounded wait). Bounded so the
/// `wait_for_change` polling loop terminates on a sane horizon even
/// if the probe never reports a change.
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
/// `prime_deadline` is the OVERALL bound for the wait — passed in by the
/// caller and NOT reset across iterations (the spec requires the prime
/// timer to be anchored to "delivery-task perspective (when flush
/// begins, not enqueue time)"). `prime_started_at` is the corresponding
/// anchor instant used for diagnostic `prime_wait_elapsed_ms` values.
/// `prime_timeout_ms` is the configured timeout value used only for
/// diagnostic inscriptions (the wait function does not use it for timing
/// decisions). `None` for both means unbounded.
///
/// Precedence: a pane that has settled into a non-prompt state with no
/// operator interaction (quiescent + not-ready + no op) is wedge
/// territory — prime_timeout MUST NOT fire in this case. Prime timeout
/// only fires while the pane is still active (changing between
/// observation ticks) or when wedge detection is disabled and the pane
/// never settles.
///
/// Returns `Ok(pane_target)` on the running branch (the pane is
/// prompt-ready + not in operator interaction); the pane target is the
/// value the probe reported in the successful observation, or an empty
/// string when the probe did not surface one. Returns
/// `Err(DeliveryWaitError::...)` on the unresponsive / wedged / failed
/// / shutdown branches.
pub fn wait_for_quiescent_three_state<W: WedgeProbe>(
    probe: &mut W,
    target_session: &str,
    quiet_window: Duration,
    prime_deadline: Option<Instant>,
    prime_started_at: Instant,
    prime_timeout_ms: Option<u64>,
    wedge_detection: bool,
) -> Result<String, DeliveryWaitError> {
    let mut state = QuiescenceState::new();
    loop {
        match quiescence_classify_step(
            probe,
            &mut state,
            target_session,
            quiet_window,
            prime_deadline,
            prime_started_at,
            prime_timeout_ms,
            wedge_detection,
        ) {
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
