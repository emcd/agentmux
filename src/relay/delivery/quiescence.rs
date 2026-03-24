use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde_json::json;

use crate::configuration::PromptReadinessTemplate;
use crate::runtime::signals::shutdown_requested;

use super::super::tmux::{
    capture_pane_snapshot, emit_delivery_diagnostic, operator_interaction_active,
    resolve_active_pane_target, resolve_cursor_column, resolve_window_activity_marker,
    sanitize_diagnostic_text,
};

const QUIET_WINDOW_MS_DEFAULT: u64 = 750;
const QUIESCENCE_TIMEOUT_MS_DEFAULT: u64 = 30_000;
const ACP_TURN_TIMEOUT_MS_DEFAULT: u64 = 120_000;
const PROMPT_INSPECT_LINES_DEFAULT: usize = 3;
const PROMPT_INSPECT_LINES_MAX: usize = 40;

#[derive(Clone, Copy, Debug)]
pub(in crate::relay) struct QuiescenceOptions {
    pub quiet_window: Duration,
    pub quiescence_timeout: Option<Duration>,
    pub acp_turn_timeout_override: Option<Duration>,
}

impl Default for QuiescenceOptions {
    fn default() -> Self {
        Self {
            quiet_window: Duration::from_millis(QUIET_WINDOW_MS_DEFAULT),
            quiescence_timeout: Some(Duration::from_millis(QUIESCENCE_TIMEOUT_MS_DEFAULT)),
            acp_turn_timeout_override: None,
        }
    }
}

impl QuiescenceOptions {
    pub(in crate::relay) fn for_sync(
        quiet_window_ms: Option<u64>,
        quiescence_timeout_ms: Option<u64>,
        acp_turn_timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            quiet_window: Duration::from_millis(
                quiet_window_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(QUIET_WINDOW_MS_DEFAULT),
            ),
            quiescence_timeout: Some(Duration::from_millis(
                quiescence_timeout_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(QUIESCENCE_TIMEOUT_MS_DEFAULT),
            )),
            acp_turn_timeout_override: acp_turn_timeout_ms
                .filter(|value| *value > 0)
                .map(Duration::from_millis),
        }
    }

    pub(in crate::relay) fn for_async(
        quiet_window_ms: Option<u64>,
        quiescence_timeout_ms: Option<u64>,
        acp_turn_timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            quiet_window: Duration::from_millis(
                quiet_window_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(QUIET_WINDOW_MS_DEFAULT),
            ),
            quiescence_timeout: quiescence_timeout_ms
                .filter(|value| *value > 0)
                .map(Duration::from_millis),
            acp_turn_timeout_override: acp_turn_timeout_ms
                .filter(|value| *value > 0)
                .map(Duration::from_millis),
        }
    }

    pub(super) fn acp_turn_timeout(
        &self,
        acp: &crate::configuration::AcpTargetConfiguration,
    ) -> Duration {
        self.acp_turn_timeout_override
            .or_else(|| acp.turn_timeout_ms.map(Duration::from_millis))
            .unwrap_or_else(|| Duration::from_millis(ACP_TURN_TIMEOUT_MS_DEFAULT))
    }
}

#[derive(Debug)]
pub(in crate::relay) enum DeliveryWaitError {
    Timeout {
        timeout: Duration,
        readiness_mismatch: bool,
        mismatch_reason: Option<String>,
    },
    Failed {
        reason: String,
    },
    Shutdown,
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

pub(super) fn wait_for_quiescent_pane(
    tmux_socket: &Path,
    target_session: &str,
    options: QuiescenceOptions,
    prompt_readiness: Option<&PromptReadinessTemplate>,
) -> Result<String, DeliveryWaitError> {
    let readiness = build_prompt_readiness_matcher(prompt_readiness)
        .map_err(|reason| DeliveryWaitError::Failed { reason })?;
    let deadline = options
        .quiescence_timeout
        .map(|timeout| Instant::now() + timeout);
    let mut readiness_mismatch = false;
    let mut mismatch_reason = None::<String>;
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

        thread::sleep(options.quiet_window);
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

        if deadline.is_some_and(|value| Instant::now() >= value) {
            let timeout = options.quiescence_timeout.unwrap_or_default();
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
