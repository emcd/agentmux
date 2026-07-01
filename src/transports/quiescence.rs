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
//! - `running` — output flowing or settled at prompt. Returns `Ok`.
//! - `unresponsive` — prime window elapsed with no observable change AND
//!   no operator interaction AND an empty inspected tail. Returns
//!   `Err(DeliveryWaitError::Timeout)`.
//! - `wedged` — pane quiesced + not prompt-ready + no operator interaction
//!   AND the inspected tail has observable non-prompt content. Returns
//!   `Err(DeliveryWaitError::Wedged)` after the counter threshold
//!   (`WEDGE_CONSECUTIVE_TICKS`) is reached.
//!
//! Operator interaction (copy-mode, active key-table, etc.) indefinitely
//! suppresses both the unresponsive and the wedged classification. The
//! prime timer does NOT fire while operator interaction is active.
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
//! The signal is the inspected tail's emptiness: a non-empty tail with
//! a non-prompt state is wedge-class; an empty tail is empty-pane. When
//! the probe supplies a [`ReadinessMismatch`] reason, that reason is
//! used directly (with the [`EMPTY_PANE_MISMATCH_PREFIX`] check applied)
//! instead of being inferred from the tail.

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
    /// state machine's `running` branch returns `Ok` when this is
    /// `true` and `operator_interaction_active` is `false`.
    pub is_prompt_ready: bool,
    /// Whether operator interaction (copy-mode, key-table, etc.) is
    /// currently active. When `true`, both the prime-timeout and
    /// wedge-class classifiers are suppressed for this iteration.
    pub operator_interaction_active: bool,
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
/// Wedge-class mismatches carry a non-empty inspection result that did
/// not match the prompt regex (e.g. stuck on a permission dialog) or a
/// cursor-column mismatch — both indicate the pane has observable
/// non-prompt content the agent is stuck on. The empty-pane mismatch
/// ([`EMPTY_PANE_MISMATCH_PREFIX`] and any `None` reason) indicates the
/// pane has NO observable content and is treated as Unresponsive, not
/// Wedged.
pub fn mismatch_is_wedge_class(mismatch_reason: &Option<String>) -> bool {
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

/// Drives the three-state delivery classifier (running / unresponsive /
/// wedged) over a [`WedgeProbe`].
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
    let _ = quiet_window;
    let mut last_mismatch_signature: Option<MismatchSignature> = None;
    let mut consecutive_quiescent_mismatches: usize = 0;

    loop {
        if shutdown_requested() {
            return Err(DeliveryWaitError::Shutdown);
        }

        // --- Observation 1 (before sleep) ---------------------------------
        let snapshot_before = probe
            .observe()
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;

        thread::sleep(quiet_window);
        if shutdown_requested() {
            return Err(DeliveryWaitError::Shutdown);
        }

        // --- Observation 2 (after sleep) ----------------------------------
        let snapshot_after = probe
            .observe()
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;

        // Quiescence: both observations agree across all signals
        // (including pane target and mismatch metadata).
        let quiescent = snapshot_before == snapshot_after;

        // `running` — pane is ready.
        if snapshot_after.is_prompt_ready && !snapshot_after.operator_interaction_active {
            emit_delivery_diagnostic(
                "delivery_ready",
                &json!({
                    "target_session": target_session,
                    "pane_target": snapshot_after.pane_target,
                }),
            );
            return Ok(snapshot_after.pane_target.unwrap_or_default());
        }

        // Operator interaction indefinitely suppresses both the unresponsive
        // (prime-timeout) and the wedged classification. Reset the wedge
        // counter so re-entry does not accumulate ticks.
        if snapshot_after.operator_interaction_active {
            emit_delivery_diagnostic(
                "delivery_operator_interaction",
                &json!({
                    "target_session": target_session,
                    "pane_target": snapshot_after.pane_target,
                    "reason": "operator_interaction_active",
                }),
            );
            consecutive_quiescent_mismatches = 0;
            continue;
        }

        let mismatch_reason = resolve_mismatch_reason(&snapshot_after);
        let wedge_class = mismatch_is_wedge_class(&mismatch_reason);

        // Track consecutive identical wedge-class non-prompt evaluations.
        // The counter increments ONLY for wedge-class mismatches; empty-pane
        // mismatches do not increment (they are Unresponsive territory,
        // not Wedged). The counter also resets whenever the wedge-class
        // signature changes (the pane transitioned through a different
        // stuck state), so transient non-prompt states (e.g. boot output
        // before the prompt appears) do not accumulate wedge ticks.
        if !snapshot_after.is_prompt_ready && quiescent && wedge_class {
            let signature = MismatchSignature::from_observation(&snapshot_after);
            match last_mismatch_signature.as_ref() {
                Some(previous) if previous == &signature => {
                    consecutive_quiescent_mismatches =
                        consecutive_quiescent_mismatches.saturating_add(1);
                }
                _ => {
                    consecutive_quiescent_mismatches = 1;
                }
            }
            last_mismatch_signature = Some(signature);
        } else {
            consecutive_quiescent_mismatches = 0;
        }

        // Wedge check: fires immediately on prime-timeout elapse when
        // the pane is showing wedge-class content, OR after the counter
        // threshold for any wedge-class mismatch even if the prime
        // window has not elapsed.
        if wedge_detection && quiescent && !snapshot_after.is_prompt_ready && wedge_class {
            let counter_fires = consecutive_quiescent_mismatches >= WEDGE_CONSECUTIVE_TICKS;
            let prime_elapsed = prime_deadline.is_some_and(|deadline| Instant::now() >= deadline);
            if counter_fires || prime_elapsed {
                emit_delivery_diagnostic(
                    "delivery_pane_wedged",
                    &json!({
                        "target_session": target_session,
                        "pane_target": snapshot_after.pane_target,
                        "mismatch_reason": mismatch_reason,
                        "consecutive_quiescent_ticks": consecutive_quiescent_mismatches,
                        "fired_via_prime_timeout": prime_elapsed && !counter_fires,
                    }),
                );
                return Err(DeliveryWaitError::Wedged {
                    reason: mismatch_reason.unwrap_or_else(|| {
                        "pane wedged at non-prompt state with no recorded mismatch reason"
                            .to_string()
                    }),
                });
            }
        }

        // Prime timeout check: hard bound on the total wait. Fires
        // `Timeout` when the prime window has elapsed AND the pane is
        // NOT showing wedge-class content. Wedge-class content takes
        // the wedge branch above; the pane is stuck, not unresponsive.
        // Operator interaction (handled earlier) indefinitely suppresses
        // both classifiers.
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
            return Err(DeliveryWaitError::Timeout {
                timeout,
                readiness_mismatch,
                mismatch_reason,
            });
        }

        // Non-quiesced or wedge not yet at threshold: emit the dedup'd
        // prompt-mismatch diagnostic if applicable.
        if !snapshot_after.is_prompt_ready {
            let signature = MismatchSignature::from_observation(&snapshot_after);
            if should_emit_prompt_mismatch(&mut last_mismatch_signature, &signature) {
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

        // Block until the probe reports a change or the prime deadline
        // elapses. This advances the probe's state for the NEXT
        // iteration (Tmux: `wait_for_change` polls tmux IPC for activity;
        // Pty: bytes arriving on the master). When the probe has nothing
        // to advance (test probe sequence exhausted, or no bytes
        // arriving), `wait_for_change` blocks until the deadline.
        if let Some(deadline) = prime_deadline {
            match probe.wait_for_change(deadline) {
                Ok(()) => {}
                Err(DeliveryWaitError::Timeout { .. }) => {
                    // No change observed before the prime deadline. The
                    // prime-timeout branch above fires on the next
                    // iteration if the deadline has elapsed.
                }
                Err(other) => return Err(other),
            }
        } else {
            // No prime timeout bound. Wait indefinitely for change.
            let unbounded_deadline = Instant::now() + Duration::from_secs(60 * 60 * 24 * 365);
            let _ = probe.wait_for_change(unbounded_deadline);
        }
    }
}
