//! The tmux [`Transport`] implementation plus the pane quiescence poll loop.
//!
//! [`TmuxTransport`] is stateless: tmux sessions are created and owned by the
//! [`lifecycle`](super::lifecycle) primitives (driven by relay bundle
//! reconcile/startup), so the transport only resolves a pane and pastes. Each
//! [`deliver`](Transport::deliver) call writes its rendered envelopes to the
//! resolved pane and returns one combined outcome; the relay replicates that
//! outcome across the coalesced tasks and records `served_successfully`
//! relay-side, keeping that relay static out of the transport.
//!
//! The quiescence poll loop (`wait_for_quiescent_pane`, module-private) lives
//! here because it is pure tmux behavior over [`pane`](super::pane) primitives.
//! It is the body of [`TmuxTransport::prepare_delivery`], the pre-delivery
//! readiness barrier: the relay hoists the wait out of `deliver` (so post-wait
//! task arrivals can coalesce into the batch) by calling `prepare_delivery`,
//! which resolves the pane and hands it back via
//! [`DeliveryPreparation`](crate::transports::DeliveryPreparation); `deliver`
//! then pastes against [`DeliveryContext::pre_resolved_target`] without
//! re-waiting. The relay-owned scheduling config (`QuiescenceOptions`) stays in
//! relay and is unpacked onto the [`DeliveryContext`] primitives the barrier
//! reads, so tmux never depends on relay.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde_json::json;

use crate::configuration::{PromptReadinessTemplate, TargetConfiguration};
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::runtime::signals::shutdown_requested;
use crate::transports::{
    DeliveryContext, DeliveryEnvelope, DeliveryPreparation, DeliveryResult, DeliveryWaitError,
    LookMode, LookSnapshotPayload, OutputView, RawWriteResult, SendOutcome, SingleDeliveryOutcome,
    StartupContext, Transport, TransportError, TransportReadiness, TransportStatus,
};

/// Default tmux look window applied when the caller omits a window size.
const LOOK_LINES_DEFAULT: usize = 120;

use super::pane::{
    capture_pane_snapshot, capture_pane_tail_lines, emit_delivery_diagnostic, inject_literal_text,
    operator_interaction_active, resolve_active_pane_target, resolve_cursor_column,
    resolve_window_activity_marker, sanitize_diagnostic_text,
};

const PROMPT_INSPECT_LINES_DEFAULT: usize = 3;
const PROMPT_INSPECT_LINES_MAX: usize = 40;
const TMUX_TARGET_UNAVAILABLE_CODE: &str = "tmux_target_unavailable";

/// Tmux pane delivery transport. Stateless — pane resolution and pasting happen
/// per `deliver` call against the runtime's tmux socket.
#[derive(Debug, Default)]
pub struct TmuxTransport;

impl TmuxTransport {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Transport for TmuxTransport {
    fn startup(&mut self, _context: StartupContext) -> Result<TransportStatus, TransportError> {
        // Tmux sessions are created and owned by the lifecycle primitives (relay
        // bundle reconcile/startup), not by the transport. There is no runtime to
        // establish here; the transport is ready to attempt delivery immediately.
        Ok(TransportStatus {
            readiness: TransportReadiness::Ready,
        })
    }

    fn prepare_delivery(
        &self,
        context: &DeliveryContext,
    ) -> Result<DeliveryPreparation, DeliveryWaitError> {
        // The relay hoists this barrier out of `deliver` so post-quiescence task
        // arrivals can coalesce into the batch. Resolve the pane once here and
        // hand it back; `deliver` then pastes against `pre_resolved_target`
        // without re-waiting. Quiescence scheduling rides on the context as
        // primitives (the relay owns `QuiescenceOptions`); the prompt-readiness
        // template comes from the tmux target member.
        let tmux_socket_path =
            tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let prompt_readiness = match &context.target_member.target {
            TargetConfiguration::Tmux(tmux_target) => tmux_target.prompt_readiness.as_ref(),
            _ => None,
        };
        let pane = wait_for_quiescent_pane(
            tmux_socket_path.as_path(),
            context.target_session.as_str(),
            context.quiet_window,
            context.quiescence_timeout,
            prompt_readiness,
        )?;
        Ok(DeliveryPreparation {
            pre_resolved_target: Some(pane),
        })
    }

    fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult {
        let target_session = context.target_session.clone();
        let message_id = envelopes
            .first()
            .map(|envelope| envelope.message_id.clone())
            .unwrap_or_default();
        let tmux_socket_path =
            tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let tmux_socket = tmux_socket_path.as_path();

        // Envelope-mode batches arrive with a pre-resolved pane (the relay hoists
        // the quiescence wait); raw input resolves the active pane here.
        let pane_target = match context.pre_resolved_target.clone() {
            Some(pane_target) => pane_target,
            None => match resolve_active_pane_target(tmux_socket, target_session.as_str()) {
                Ok(pane_target) => pane_target,
                Err(reason) => {
                    return single_result(SingleDeliveryOutcome {
                        target_session,
                        message_id,
                        outcome: SendOutcome::Failed,
                        reason_code: Some(TMUX_TARGET_UNAVAILABLE_CODE.to_string()),
                        reason: Some(reason),
                        details: None,
                    });
                }
            },
        };

        let mut failed_reason = None::<String>;
        for envelope in &envelopes {
            if let Err(reason) = inject_literal_text(
                tmux_socket,
                &pane_target,
                envelope.rendered.as_str(),
                envelope.append_enter,
            ) {
                failed_reason = Some(reason);
                break;
            }
        }

        let outcome = match failed_reason {
            None => SingleDeliveryOutcome {
                target_session,
                message_id,
                outcome: SendOutcome::Delivered,
                reason_code: None,
                reason: None,
                details: None,
            },
            Some(reason) => SingleDeliveryOutcome {
                target_session,
                message_id,
                outcome: SendOutcome::Failed,
                reason_code: None,
                reason: Some(reason),
                details: None,
            },
        };
        single_result(outcome)
    }

    fn is_ready(&self) -> bool {
        // Tmux has no per-target runtime to gate on; pane resolution surfaces
        // unavailability per delivery instead.
        true
    }

    fn raw_write(
        &mut self,
        text: &str,
        append_enter: bool,
        context: &DeliveryContext,
    ) -> RawWriteResult {
        let tmux_socket_path =
            tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let tmux_socket = tmux_socket_path.as_path();
        let pane_target =
            match resolve_active_pane_target(tmux_socket, context.target_session.as_str()) {
                Ok(pane_target) => pane_target,
                Err(reason) => return RawWriteResult::Failed { reason },
            };
        match inject_literal_text(tmux_socket, &pane_target, text, append_enter) {
            Ok(()) => RawWriteResult::Written,
            Err(reason) => RawWriteResult::Failed { reason },
        }
    }

    fn shutdown(&mut self) {
        // Stateless: no runtime to tear down. Sessions outlive the transport and
        // are reaped by the lifecycle primitives on bundle shutdown.
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        // Tmux output is not worker-owned: a look is a stateless socket capture
        // (see `TmuxOutputView`), valid independent of worker lifecycle, so the
        // relay accessor config-constructs the view rather than reading a
        // published handle. A future stateful/streaming tmux worker would
        // publish here instead.
        None
    }
}

/// A config-constructed [`OutputView`] over a tmux session's active pane.
///
/// Unlike the ACP view, this holds no worker-owned state: it captures the tmux
/// pane directly through the socket, so it is valid before any delivery has
/// spawned a worker for the session. The relay's `get_output_view` accessor
/// constructs it from the socket path and session id.
pub struct TmuxOutputView {
    socket_path: PathBuf,
    session_id: String,
}

impl TmuxOutputView {
    /// Builds a view over the active pane of `session_id` on `socket_path`.
    #[must_use]
    pub fn new(socket_path: PathBuf, session_id: String) -> Self {
        Self {
            socket_path,
            session_id,
        }
    }
}

impl OutputView for TmuxOutputView {
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        // Tmux has no offset semantics; reject a non-zero offset as a validation
        // error the relay surfaces, rather than silently ignoring it.
        if mode.offset.unwrap_or(0) > 0 {
            return Err(TransportError {
                code: "validation_offset_unsupported".to_string(),
                reason: "offset is only supported for ACP look targets".to_string(),
                details: Some(json!({ "offset": mode.offset })),
            });
        }
        let requested_lines = mode
            .lines
            .map(|lines| lines as usize)
            .unwrap_or(LOOK_LINES_DEFAULT);
        let pane_target =
            resolve_active_pane_target(self.socket_path.as_path(), self.session_id.as_str())
                .map_err(|reason| TransportError {
                    code: "internal_unexpected_failure".to_string(),
                    reason: "failed to resolve active pane for look target".to_string(),
                    details: Some(json!({ "cause": reason })),
                })?;
        let snapshot_lines = capture_pane_tail_lines(
            self.socket_path.as_path(),
            pane_target.as_str(),
            requested_lines,
        )
        .map_err(|reason| TransportError {
            code: "internal_unexpected_failure".to_string(),
            reason: "failed to capture look snapshot".to_string(),
            details: Some(json!({ "cause": reason })),
        })?;
        Ok(LookSnapshotPayload::Lines { snapshot_lines })
    }
}

/// Wraps a single combined outcome as a [`DeliveryResult`]; tmux produces one
/// outcome per `deliver` call which the relay fans out across the batch.
fn single_result(outcome: SingleDeliveryOutcome) -> DeliveryResult {
    DeliveryResult {
        outcomes: vec![outcome],
    }
}

#[derive(Debug)]
struct PromptReadinessMatcher {
    prompt_regex: Regex,
    inspect_lines: usize,
    input_idle_cursor_column: Option<usize>,
}

#[derive(Debug, Default)]
struct PromptReadinessEvaluation {
    ready: bool,
    mismatch_reason: Option<String>,
    inspected_block: Option<String>,
    regex_matched: Option<bool>,
    expected_cursor_column: Option<usize>,
    observed_cursor_column: Option<usize>,
}

/// Signature of a non-ready evaluation used to dedup `delivery_prompt_mismatch`
/// log lines emitted from the quiescence wait. When the pane is stuck on the
/// same non-matching state (for example a Claude Code tool-approval dialog
/// that the readiness regex does not match), repeated identical evaluations
/// across poll ticks collapse to a single inscription. The dialog is still
/// treated as non-quiescent and delivery still blocks until the state clears.
#[derive(Debug, PartialEq, Eq)]
struct PromptMismatchSignature {
    mismatch_reason: Option<String>,
    inspected_block: Option<String>,
    regex_matched: Option<bool>,
    expected_cursor_column: Option<usize>,
    observed_cursor_column: Option<usize>,
}

impl PromptMismatchSignature {
    fn from_evaluation(evaluation: &PromptReadinessEvaluation) -> Self {
        Self {
            mismatch_reason: evaluation.mismatch_reason.clone(),
            inspected_block: evaluation.inspected_block.clone(),
            regex_matched: evaluation.regex_matched,
            expected_cursor_column: evaluation.expected_cursor_column,
            observed_cursor_column: evaluation.observed_cursor_column,
        }
    }
}

/// Returns whether a mismatch evaluation should emit a fresh diagnostic. The
/// first call after entering the wait, and every call whose evaluation
/// signature differs from the last emitted one, returns `true` and updates
/// `last`. Repeated identical signatures return `false`.
fn should_emit_prompt_mismatch(
    last: &mut Option<PromptMismatchSignature>,
    evaluation: &PromptReadinessEvaluation,
) -> bool {
    let signature = PromptMismatchSignature::from_evaluation(evaluation);
    if last.as_ref() == Some(&signature) {
        false
    } else {
        *last = Some(signature);
        true
    }
}

/// Blocks until the target's active pane is quiescent (and, if configured,
/// matches the prompt-readiness template), returning the resolved pane.
///
/// Takes the quiescence parameters as primitives — the relay owns the
/// `QuiescenceOptions` config and unpacks it onto the [`DeliveryContext`], which
/// [`TmuxTransport::prepare_delivery`] forwards here — so this loop depends only
/// on `crate::tmux::pane`, `crate::configuration`, and `crate::runtime`, never on
/// `crate::relay`. It is a private detail of the tmux barrier; the relay reaches
/// it only through [`Transport::prepare_delivery`].
fn wait_for_quiescent_pane(
    tmux_socket: &Path,
    target_session: &str,
    quiet_window: Duration,
    quiescence_timeout: Option<Duration>,
    prompt_readiness: Option<&PromptReadinessTemplate>,
) -> Result<String, DeliveryWaitError> {
    let readiness = build_prompt_readiness_matcher(prompt_readiness)
        .map_err(|reason| DeliveryWaitError::Failed { reason })?;
    let deadline = quiescence_timeout.map(|timeout| Instant::now() + timeout);
    let mut readiness_mismatch = false;
    let mut mismatch_reason = None::<String>;
    let mut last_mismatch_signature: Option<PromptMismatchSignature> = None;
    loop {
        if shutdown_requested() {
            return Err(DeliveryWaitError::Shutdown);
        }
        let pane_before = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_before = capture_pane_snapshot(tmux_socket, &pane_before)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let activity_before = resolve_window_activity_marker(tmux_socket, &pane_before)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;

        thread::sleep(quiet_window);
        if shutdown_requested() {
            return Err(DeliveryWaitError::Shutdown);
        }

        let pane_after = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_after = capture_pane_snapshot(tmux_socket, &pane_after)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let activity_after = resolve_window_activity_marker(tmux_socket, &pane_after)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let pane_is_quiescent = pane_before == pane_after
            && snapshot_before == snapshot_after
            && match (activity_before.as_ref(), activity_after.as_ref()) {
                (Some(before), Some(after)) => before == after,
                _ => true,
            };
        if pane_is_quiescent {
            if let Some(reason) =
                operator_interaction_active(tmux_socket, target_session, pane_after.as_str())
                    .map_err(|reason| DeliveryWaitError::Failed { reason })?
            {
                emit_delivery_diagnostic(
                    "delivery_operator_interaction",
                    &json!({
                        "target_session": target_session,
                        "pane_target": pane_after,
                        "reason": reason,
                    }),
                );
                continue;
            }
            let evaluation = match prompt_readiness_matches(
                tmux_socket,
                pane_after.as_str(),
                snapshot_after.as_str(),
                readiness.as_ref(),
            ) {
                Ok(evaluation) => evaluation,
                Err(reason) => return Err(DeliveryWaitError::Failed { reason }),
            };
            if evaluation.ready {
                emit_delivery_diagnostic(
                    "delivery_ready",
                    &json!({
                        "target_session": target_session,
                        "pane_target": pane_after,
                    }),
                );
                return Ok(pane_after);
            }
            readiness_mismatch = true;
            mismatch_reason = evaluation.mismatch_reason.clone();
            if should_emit_prompt_mismatch(&mut last_mismatch_signature, &evaluation) {
                emit_delivery_diagnostic(
                    "delivery_prompt_mismatch",
                    &json!({
                        "target_session": target_session,
                        "pane_target": pane_after,
                        "mismatch_reason": evaluation.mismatch_reason,
                        "regex_matched": evaluation.regex_matched,
                        "inspected_block": evaluation.inspected_block,
                        "expected_cursor_column": evaluation.expected_cursor_column,
                        "observed_cursor_column": evaluation.observed_cursor_column,
                    }),
                );
            }
        }

        if deadline.is_some_and(|value| Instant::now() >= value) {
            let timeout = quiescence_timeout.unwrap_or_default();
            emit_delivery_diagnostic(
                "quiescence_timeout",
                &json!({
                    "target_session": target_session,
                    "quiescence_timeout_ms": timeout.as_millis(),
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
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // Dedup is a private detail of `wait_for_quiescent_pane`: the loop owns
    // the signature state and emits via a crate-private helper. Driving it
    // from an external test would require either widening visibility on the
    // helper / signature struct or spinning up tmux to drive the loop. One
    // inline unit covers the three transitions that matter: first emit,
    // identical repeat suppressed, and signature change re-emits.
    #[test]
    fn dedup_emits_only_on_signature_transitions() {
        let stuck = PromptReadinessEvaluation {
            mismatch_reason: Some("prompt regex did not match inspected pane tail".to_string()),
            inspected_block: Some("Do you want to proceed?".to_string()),
            regex_matched: Some(false),
            expected_cursor_column: Some(4),
            observed_cursor_column: None,
            ..PromptReadinessEvaluation::default()
        };
        let cursor_only = PromptReadinessEvaluation {
            mismatch_reason: Some("cursor column 0 did not match required 4".to_string()),
            inspected_block: Some("> ".to_string()),
            regex_matched: Some(true),
            expected_cursor_column: Some(4),
            observed_cursor_column: Some(0),
            ..PromptReadinessEvaluation::default()
        };

        let mut last = None;
        assert!(
            should_emit_prompt_mismatch(&mut last, &stuck),
            "first mismatch must emit",
        );
        assert!(
            !should_emit_prompt_mismatch(&mut last, &stuck),
            "identical follow-up must suppress",
        );
        assert!(
            !should_emit_prompt_mismatch(&mut last, &stuck),
            "second identical follow-up must suppress",
        );
        assert!(
            should_emit_prompt_mismatch(&mut last, &cursor_only),
            "signature change must re-emit",
        );
        assert!(
            !should_emit_prompt_mismatch(&mut last, &cursor_only),
            "post-change identical follow-up must suppress",
        );
    }
}
