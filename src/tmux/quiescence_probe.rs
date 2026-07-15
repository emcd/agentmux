//! Tmux quiescence probe and prompt-readiness matching.
//!
//! This module owns the [`PaneQuiescenceProbe`] trait (the transport-internal
//! seam the tmux delivery task uses to observe pane quiescence + prompt
//! readiness), the real tmux-backed implementation, the
//! [`TmuxAsWedgeProbe`](struct.TmuxAsWedgeProbe.html) adapter that exposes a
//! probe as the cross-transport [`WedgeProbe`], and the prompt-readiness
//! matching logic that classifies whether the inspected pane tail matches the
//! configured prompt regex + cursor column. [`PromptReadinessEvaluation`] is
//! the structured result the wait loop consumes.
//!
//! Public surface (re-exported from [`super`]):
//! - [`PromptReadinessEvaluation`] — the structured readiness classification
//!   returned by [`PaneQuiescenceProbe::next_evaluation`].
//! - [`PaneQuiescenceProbe`] — the test seam the external test surface in
//!   `tests/unit/tmux_transport.rs` injects scripted probes into.
//! - [`wait_for_quiescent_pane_three_state`] — the three-state classifier
//!   entry point (running / unresponsive / wedged) the delivery task calls.
//!
//! Crate-private surface:
//! - [`RealPaneQuiescenceProbe`] — the tmux-backed probe used at runtime.
//! - [`build_prompt_readiness_matcher`], [`prompt_readiness_matches`] — the
//!   prompt-readiness matching helpers.
//!
//! The cross-transport [`WedgeProbe`] and the dedup helper
//! `should_emit_prompt_mismatch` live in `src/transports/quiescence.rs` (see
//! commit `8e50657`, which lifted the wedge/prime state machine into the
//! shared module); this module only owns the tmux-specific seam.

use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use regex::Regex;

use crate::configuration::PromptReadinessTemplate;
use crate::runtime::signals::shutdown_requested;
use crate::transports::{
    DeliveryWaitError, WedgeObservation, WedgeProbe, wait_for_quiescent_three_state,
};

use super::pane::{
    capture_pane_snapshot, resolve_active_pane_target, resolve_cursor_column,
    resolve_window_activity_marker, sanitize_diagnostic_text,
};

const PROMPT_INSPECT_LINES_DEFAULT: usize = 3;
const PROMPT_INSPECT_LINES_MAX: usize = 40;

// ---------------------------------------------------------------------------
// Quiescence poll loop
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PromptReadinessMatcher {
    prompt_regex: Regex,
    inspect_lines: usize,
    input_idle_cursor_column: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptReadinessEvaluation {
    pub ready: bool,
    pub mismatch_reason: Option<String>,
    pub inspected_block: Option<String>,
    pub regex_matched: Option<bool>,
    pub expected_cursor_column: Option<usize>,
    pub observed_cursor_column: Option<usize>,
}

/// Transport-internal seam for the tmux quiescence wait.
///
/// The real implementation ([`RealPaneQuiescenceProbe`]) wraps tmux queries
/// against the active pane. Tests inject scripted probes that drive the
/// three-state classifier deterministically — see the unit tests in
/// `tests/unit/tmux_transport.rs` for the four probe classes
/// (unresponsive, wedged, slow-prompt, normal).
///
/// `pub` to support the external test surface; the trait is not part of
/// the public runtime API (no other code outside `src/tmux` consumes it).
pub trait PaneQuiescenceProbe: Send {
    /// Resolves the current prompt-readiness evaluation for the target pane.
    /// The wait loop calls this twice per quiescence check (with a
    /// `quiet_window` sleep between) and compares results.
    fn next_evaluation(&mut self) -> Result<PromptReadinessEvaluation, String>;

    /// Resolves the active pane target for the target session (e.g. `%0`).
    /// Used by the wait loop to record the pane on terminal outcomes and to
    /// thread through to the wedge inscription event.
    fn resolve_active_pane(&mut self) -> Result<String, String>;

    /// Resolves the pane's terminal-output-write marker (Tmux's
    /// `#{window_activity}`) as a `u64` epoch-seconds value. The
    /// `quiescence_classify_step` cross-transport classifier compares
    /// this field between two consecutive observations to detect
    /// whether bytes were written to the terminal during the
    /// `quiet_window` (a positive "output is flowing" signal); an
    /// advance suppresses the wedge / unresponsive / delivered
    /// classifications via the Busy pre-classification. Returns
    /// `Ok(Some(0))` (constant activity, no advance possible) when
    /// the format is unavailable on the running tmux version or
    /// when the marker is unparseable; the cross-transport
    /// classifier treats this as "no activity signal available,"
    /// falling back to pre-change behavior for that probe.
    fn last_window_activity_marker(&mut self) -> Result<Option<u64>, String>;

    /// Blocks until the pane shows a change (the next observation differs
    /// from the previous one) or the supplied `deadline` elapses. Returns
    /// `Ok(())` on observed change; `Err(DeliveryWaitError::Timeout)` on
    /// deadline elapsed with no change; `Err(DeliveryWaitError::Failed)`
    /// on probe errors. The wait loop passes a deadline derived from the
    /// per-coder `prime_timeout_ms` so the probe bounds its wait by the
    /// same prime window the loop tracks.
    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError>;
}

/// Real [`PaneQuiescenceProbe`] backed by tmux queries. Holds the socket path
/// and target session id used by every observation; the underlying tmux
/// queries are the same primitives the legacy wait loop called directly.
pub(crate) struct RealPaneQuiescenceProbe<'a> {
    tmux_socket: &'a Path,
    target_session: &'a str,
    matcher: Option<PromptReadinessMatcher>,
}

impl<'a> RealPaneQuiescenceProbe<'a> {
    pub(crate) fn new(
        tmux_socket: &'a Path,
        target_session: &'a str,
        prompt_readiness: Option<&PromptReadinessTemplate>,
    ) -> Result<Self, DeliveryWaitError> {
        let matcher = build_prompt_readiness_matcher(prompt_readiness)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        Ok(Self {
            tmux_socket,
            target_session,
            matcher,
        })
    }
}

impl PaneQuiescenceProbe for RealPaneQuiescenceProbe<'_> {
    fn next_evaluation(&mut self) -> Result<PromptReadinessEvaluation, String> {
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)?;
        let snapshot = capture_pane_snapshot(self.tmux_socket, &pane_target)?;
        prompt_readiness_matches(
            self.tmux_socket,
            pane_target.as_str(),
            snapshot.as_str(),
            self.matcher.as_ref(),
        )
    }

    fn resolve_active_pane(&mut self) -> Result<String, String> {
        resolve_active_pane_target(self.tmux_socket, self.target_session)
    }

    fn last_window_activity_marker(&mut self) -> Result<Option<u64>, String> {
        // Re-query tmux for `#{window_activity}` at observation
        // time. The existing `resolve_window_activity_marker`
        // returns `Ok(None)` when the format is unavailable on the
        // running tmux version; we surface that as `Some(0)` so
        // the cross-transport classifier's Busy pre-classification
        // is silently disabled (constant activity => no comparator
        // advance => Busy never fires), preserving pre-change
        // behavior for older tmux versions.
        //
        // The parsed `u64` is the project-defined surface — the
        // cross-transport classifier uses it as a monotonic
        // comparison value, not as a wall-clock timestamp. Tmux's
        // `#{window_activity}` returns seconds-since-epoch on
        // modern versions, which is naturally monotonic within a
        // session lifetime.
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)?;
        let marker = resolve_window_activity_marker(self.tmux_socket, &pane_target)?;
        let value = marker
            .as_deref()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        Ok(Some(value))
    }

    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError> {
        // Sleep in short slices, polling the activity marker and pane
        // target. Returns as soon as either changes (or the deadline
        // elapses).
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let mut last_activity = resolve_window_activity_marker(self.tmux_socket, &pane_target)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let mut last_snapshot = capture_pane_snapshot(self.tmux_socket, &pane_target)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        loop {
            if shutdown_requested() {
                return Err(DeliveryWaitError::Shutdown);
            }
            if Instant::now() >= deadline {
                return Err(DeliveryWaitError::Timeout {
                    timeout: deadline.saturating_duration_since(Instant::now()),
                    readiness_mismatch: false,
                    mismatch_reason: None,
                });
            }
            // Keep the slice short so shutdown_requested is observed promptly.
            thread::sleep(Duration::from_millis(50));
            let pane_target_now = resolve_active_pane_target(self.tmux_socket, self.target_session)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if pane_target_now != pane_target {
                return Ok(());
            }
            let activity_now = resolve_window_activity_marker(self.tmux_socket, &pane_target_now)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if activity_now != last_activity {
                return Ok(());
            }
            let snapshot_now = capture_pane_snapshot(self.tmux_socket, &pane_target_now)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if snapshot_now != last_snapshot {
                return Ok(());
            }
            last_activity = activity_now;
            last_snapshot = snapshot_now;
        }
    }
}

/// Adapter that exposes a [`PaneQuiescenceProbe`] as the cross-transport
/// [`WedgeProbe`]. Constructed per quiescence iteration by
/// [`wait_for_quiescent_pane_three_state`]; holds a `&mut` borrow so it
/// does not own the underlying probe.
///
/// The adapter calls `next_evaluation()` exactly once per
/// [`observe`](WedgeProbe::observe) call. This keeps each
/// quiescence iteration to two `observe()` calls (= two
/// `next_evaluation()` roundtrips), matching the legacy
/// `wait_for_quiescent_pane_three_state` call frequency. Scripted test
/// probes with `abort_after_calls` thresholds trip at the iteration count
/// the test expects rather than at 4x that count.
///
/// Pane target resolution is delegated to the underlying probe (which
/// returns the active tmux pane id like `%0`) so the state machine can
/// thread it through to its diagnostic inscriptions
/// (`delivery_ready`, `delivery_pane_wedged`, `delivery_prime_timeout`,
/// `delivery_prompt_mismatch`).
struct TmuxAsWedgeProbe<'a, P: PaneQuiescenceProbe> {
    inner: &'a mut P,
}

impl<'a, P: PaneQuiescenceProbe> TmuxAsWedgeProbe<'a, P> {
    fn new(inner: &'a mut P) -> Self {
        Self { inner }
    }
}

impl<'a, P: PaneQuiescenceProbe> WedgeProbe for TmuxAsWedgeProbe<'a, P> {
    fn observe(&mut self) -> Result<WedgeObservation, String> {
        let evaluation = self.inner.next_evaluation()?;
        let pane_target = self.inner.resolve_active_pane()?;
        let activity_generation = self.inner.last_window_activity_marker()?.unwrap_or(0);
        let mismatch = if evaluation.ready {
            None
        } else {
            Some(crate::transports::ReadinessMismatch {
                reason: evaluation.mismatch_reason.clone().unwrap_or_default(),
                regex_matched: evaluation.regex_matched,
                expected_cursor_column: evaluation
                    .expected_cursor_column
                    .and_then(|c| u16::try_from(c).ok()),
                observed_cursor_column: evaluation
                    .observed_cursor_column
                    .and_then(|c| u16::try_from(c).ok()),
            })
        };
        Ok(WedgeObservation {
            inspected_tail: evaluation.inspected_block.unwrap_or_default(),
            is_prompt_ready: evaluation.ready,
            pane_target: Some(pane_target),
            mismatch,
            activity_generation,
        })
    }

    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError> {
        self.inner.wait_for_change(deadline)
    }
}

/// Drives the three-state delivery classifier (running / unresponsive /
/// wedged) over a [`PaneQuiescenceProbe`]. `pub` to support the external
/// test surface in `tests/unit/tmux_transport.rs`; the function is not part
/// of the runtime API (callers reach it via `flush_and_resolve`).
///
/// Three-state classifier:
/// - `running` — output flowing or settled at prompt. Returns `Ok(pane)`.
/// - `unresponsive` — prime window elapsed with no observable change AND
///   no operator interaction. Returns `Err(DeliveryWaitError::Timeout)`.
/// - `wedged` — pane quiesced + not prompt-ready + no operator interaction.
///   Returns `Err(DeliveryWaitError::Wedged)` when `wedge_detection` is
///   enabled; otherwise the loop continues waiting (the prime window is
///   the only bounded-wait path).
///
/// Operator interaction (copy-mode or active key-table) indefinitely
/// suppresses BOTH the unresponsive and the wedged classification, on
/// both the prime window and the post-quiescence wait. Prime timeout
/// does NOT fire while operator interaction is active.
///
/// This is a thin wrapper that constructs a [`TmuxAsWedgeProbe`] adapter
/// and delegates to the cross-transport
/// [`wait_for_quiescent_three_state`] in `src/transports/quiescence.rs`.
/// The signature is preserved (including the `Result<String,
/// DeliveryWaitError>` return type that callers and unit tests rely on);
/// the pane target in the `Ok` value comes from the post-wait
/// observation the state machine reports (which differs from the
/// pre-wait pane target when the active pane changed during the wait).
/// The 16-probe test surface in `tests/unit/tmux_transport.rs` is
/// unchanged — probes implement [`PaneQuiescenceProbe`] as before.
///
/// `prime_deadline`, `prime_started_at`, `prime_timeout_ms`, and
/// `wedge_detection` carry the same semantics as the underlying
/// [`wait_for_quiescent_three_state`] (see that function's docs).
pub fn wait_for_quiescent_pane_three_state<P: PaneQuiescenceProbe>(
    probe: &mut P,
    target_session: &str,
    quiet_window: Duration,
    prime_deadline: Option<Instant>,
    prime_started_at: Instant,
    prime_timeout_ms: Option<u64>,
    wedge_detection: bool,
) -> Result<String, DeliveryWaitError> {
    let mut adapter = TmuxAsWedgeProbe::new(probe);
    wait_for_quiescent_three_state(
        &mut adapter,
        target_session,
        quiet_window,
        prime_deadline,
        prime_started_at,
        prime_timeout_ms,
        wedge_detection,
    )
}

fn build_prompt_readiness_matcher(
    template: Option<&PromptReadinessTemplate>,
) -> Result<Option<PromptReadinessMatcher>, String> {
    let Some(template) = template else {
        return Ok(None);
    };

    let prompt_regex = Regex::new(template.prompt_regex.as_str())
        .map_err(|source| format!("invalid prompt_readiness.prompt_regex: {source}"))?;
    let inspect_lines = template
        .inspect_lines
        .unwrap_or(PROMPT_INSPECT_LINES_DEFAULT)
        .clamp(1, PROMPT_INSPECT_LINES_MAX);

    Ok(Some(PromptReadinessMatcher {
        prompt_regex,
        inspect_lines,
        input_idle_cursor_column: template.input_idle_cursor_column,
    }))
}

fn prompt_readiness_matches(
    tmux_socket: &Path,
    pane_target: &str,
    snapshot: &str,
    matcher: Option<&PromptReadinessMatcher>,
) -> Result<PromptReadinessEvaluation, String> {
    let Some(matcher) = matcher else {
        return Ok(PromptReadinessEvaluation {
            ready: true,
            ..PromptReadinessEvaluation::default()
        });
    };

    let inspected = snapshot
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .take(matcher.inspect_lines)
        .collect::<Vec<_>>();
    if inspected.is_empty() {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(
                "inspected pane tail was empty after trimming trailing blank lines".to_string(),
            ),
            regex_matched: Some(false),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }
    let mut ordered = inspected;
    ordered.reverse();
    let block = ordered.join("\n");
    if !matcher.prompt_regex.is_match(block.as_str()) {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some("prompt regex did not match inspected pane tail".to_string()),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(false),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }

    let Some(expected_cursor_column) = matcher.input_idle_cursor_column else {
        return Ok(PromptReadinessEvaluation {
            ready: true,
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            ..PromptReadinessEvaluation::default()
        });
    };
    let cursor_column = resolve_cursor_column(tmux_socket, pane_target)?;
    if cursor_column != expected_cursor_column {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(format!(
                "cursor column {} did not match required {}",
                cursor_column, expected_cursor_column
            )),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            expected_cursor_column: Some(expected_cursor_column),
            observed_cursor_column: Some(cursor_column),
            ..PromptReadinessEvaluation::default()
        });
    }

    Ok(PromptReadinessEvaluation {
        ready: true,
        inspected_block: Some(sanitize_diagnostic_text(&block)),
        regex_matched: Some(true),
        expected_cursor_column: Some(expected_cursor_column),
        observed_cursor_column: Some(cursor_column),
        ..PromptReadinessEvaluation::default()
    })
}
