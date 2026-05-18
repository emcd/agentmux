use std::collections::{HashSet, VecDeque};

use crate::relay::{
    ChatOutcome, ChatResult, ChatStatus, RelayRequest, RelayResponse, RelayStreamEvent,
};
use crate::runtime::error::RuntimeError;

use super::{
    AppState, ChatHistoryDirection, ChatHistoryEntry, PendingPermissionEntry,
    PendingPermissionOption, SEEN_STREAM_IDS_MAXIMUM, map_relay_error, merge_tui_targets,
};

impl AppState {
    pub fn send_message(&mut self) -> Result<(), RuntimeError> {
        if self.message_field.trim().is_empty() {
            return Err(RuntimeError::validation(
                "validation_missing_message_input",
                "message body is required",
            ));
        }
        let targets = merge_tui_targets(&self.to_field, &self.bundle_name)?;
        let message_body = self.message_field.clone();
        let response = self.request_relay(&RelayRequest::Chat {
            request_id: None,
            sender_session: self.sender_session.clone(),
            message: message_body.clone(),
            targets,
            broadcast: false,
            quiet_window_ms: None,
            quiescence_timeout_ms: None,
            acp_turn_timeout_ms: None,
        })?;
        match response {
            RelayResponse::Chat {
                status, results, ..
            } => {
                let history_targets = results
                    .iter()
                    .map(|result| result.target_session.clone())
                    .collect::<Vec<_>>();
                if !history_targets.is_empty() {
                    self.push_outgoing_chat_history(&history_targets, message_body.as_str());
                }
                self.record_chat_events(&status, &results);
                self.push_status(
                    None,
                    format!(
                        "send accepted status={} pending={}",
                        render_chat_status(&status),
                        self.pending_deliveries_count()
                    ),
                );
                self.clear_compose_fields();
                self.relay_stream_poll_error_reported = false;
                Ok(())
            }
            RelayResponse::Error { error } => Err(map_relay_error(error)),
            other => Err(RuntimeError::validation(
                "internal_unexpected_failure",
                format!("relay returned unexpected response variant: {other:?}"),
            )),
        }
    }

    pub fn set_chat_history_viewport_height(&mut self, height: usize) {
        self.chat_history_viewport_height = height.max(1);
        self.clamp_chat_history_scroll();
    }

    pub fn set_chat_history_total_lines(&mut self, total_lines: usize) {
        self.chat_history_total_lines = total_lines;
        self.clamp_chat_history_scroll();
    }

    pub fn chat_history_scroll(&self) -> usize {
        self.chat_history_scroll
    }

    /// Window of rendered chat-history lines (`start..end`, oldest-to-newest)
    /// that the viewport should display for the current scroll offset.
    pub fn chat_history_line_window(&self) -> (usize, usize) {
        let viewport = self.chat_history_viewport_height.max(1);
        let total = self.chat_history_total_lines;
        let scroll = self.chat_history_scroll.min(total.saturating_sub(viewport));
        let end = total.saturating_sub(scroll);
        let start = end.saturating_sub(viewport);
        (start, end)
    }

    pub fn scroll_chat_history_page_up(&mut self) {
        let page = self.chat_history_viewport_height.max(1);
        let max_scroll = self.max_chat_history_scroll();
        self.chat_history_scroll = (self.chat_history_scroll + page).min(max_scroll);
    }

    pub fn scroll_chat_history_page_down(&mut self) {
        let page = self.chat_history_viewport_height.max(1);
        self.chat_history_scroll = self.chat_history_scroll.saturating_sub(page);
    }

    pub fn scroll_chat_history_up(&mut self) {
        let max_scroll = self.max_chat_history_scroll();
        self.chat_history_scroll = (self.chat_history_scroll + 1).min(max_scroll);
    }

    pub fn scroll_chat_history_down(&mut self) {
        self.chat_history_scroll = self.chat_history_scroll.saturating_sub(1);
    }

    pub fn snap_chat_history_to_latest(&mut self) {
        self.chat_history_scroll = 0;
    }

    pub fn pending_deliveries_count(&self) -> usize {
        self.pending_delivery_ids.len()
    }

    pub(super) fn push_event(&mut self, event: impl Into<String>) {
        self.event_history.push_front(event.into());
        while self.event_history.len() > super::EVENT_HISTORY_MAXIMUM {
            self.event_history.pop_back();
        }
    }

    pub(super) fn push_chat_history_entry(&mut self, entry: ChatHistoryEntry) {
        self.chat_history.push_front(entry);
        while self.chat_history.len() > super::CHAT_HISTORY_MAXIMUM {
            self.chat_history.pop_back();
        }
        self.chat_history_scroll = 0;
    }

    pub(super) fn push_outgoing_chat_history(&mut self, targets: &[String], body: &str) {
        let peer_session = targets.join(", ");
        self.push_chat_history_entry(ChatHistoryEntry {
            direction: ChatHistoryDirection::Outgoing,
            peer_session,
            body: body.trim_end_matches('\n').to_string(),
        });
    }

    fn push_incoming_chat_history(&mut self, sender_session: &str, body: &str) {
        self.push_chat_history_entry(ChatHistoryEntry {
            direction: ChatHistoryDirection::Incoming,
            peer_session: sender_session.to_string(),
            body: body.to_string(),
        });
    }

    fn max_chat_history_scroll(&self) -> usize {
        let visible = self.chat_history_viewport_height.max(1);
        self.chat_history_total_lines.saturating_sub(visible)
    }

    fn clamp_chat_history_scroll(&mut self) {
        let max_scroll = self.max_chat_history_scroll();
        self.chat_history_scroll = self.chat_history_scroll.min(max_scroll);
    }

    fn remember_seen_id(
        seen: &mut HashSet<String>,
        order: &mut VecDeque<String>,
        key: impl Into<String>,
    ) -> bool {
        let key = key.into();
        if seen.contains(&key) {
            return false;
        }
        seen.insert(key.clone());
        order.push_back(key);
        while order.len() > SEEN_STREAM_IDS_MAXIMUM {
            if let Some(evicted) = order.pop_front() {
                seen.remove(evicted.as_str());
            }
        }
        true
    }

    pub(super) fn record_chat_events(&mut self, status: &ChatStatus, results: &[ChatResult]) {
        let mut accepted_count = 0usize;
        for result in results {
            match result.outcome {
                ChatOutcome::Queued => {
                    if self
                        .terminal_delivery_message_ids
                        .contains(result.message_id.as_str())
                    {
                        continue;
                    }
                    self.pending_delivery_ids.insert(result.message_id.clone());
                    accepted_count += 1;
                }
                _ => {
                    self.pending_delivery_ids.remove(&result.message_id);
                }
            }
        }

        self.push_event(format!(
            "send status={} targets={} accepted={} pending={}",
            render_chat_status(status),
            results.len(),
            accepted_count,
            self.pending_deliveries_count()
        ));

        for result in results {
            let (outcome, reason_code) = map_chat_result_outcome(&result.outcome);
            if let Some(reason) = result.reason.as_deref() {
                self.push_event(format!(
                    "target={} outcome={} reason_code={} message_id={} reason={}",
                    result.target_session,
                    outcome,
                    reason_code.unwrap_or("-"),
                    result.message_id,
                    reason
                ));
            } else {
                self.push_event(format!(
                    "target={} outcome={} reason_code={} message_id={}",
                    result.target_session,
                    outcome,
                    reason_code.unwrap_or("-"),
                    result.message_id
                ));
            }
        }
    }

    pub(super) fn record_stream_events(&mut self, events: &[RelayStreamEvent]) {
        for event in events {
            match event.event_type.as_str() {
                "incoming_message" => {
                    let sender_session = event
                        .payload
                        .get("sender_session")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("<unknown>");
                    let message_id = event
                        .payload
                        .get("message_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("<unknown>");
                    let body = event
                        .payload
                        .get("body")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if message_id != "<unknown>"
                        && !Self::remember_seen_id(
                            &mut self.seen_incoming_message_ids,
                            &mut self.seen_incoming_message_order,
                            message_id,
                        )
                    {
                        continue;
                    }
                    self.push_incoming_chat_history(sender_session, body);
                    self.push_event(format!(
                        "incoming target={} sender={} message_id={}",
                        event.target_session, sender_session, message_id
                    ));
                }
                "delivery_outcome" => {
                    let message_id = event
                        .payload
                        .get("message_id")
                        .and_then(serde_json::Value::as_str);
                    let relay_phase = event
                        .payload
                        .get("phase")
                        .and_then(serde_json::Value::as_str);
                    let relay_outcome = event
                        .payload
                        .get("outcome")
                        .and_then(serde_json::Value::as_str);
                    let relay_reason_code = event
                        .payload
                        .get("reason_code")
                        .and_then(serde_json::Value::as_str);
                    if let Some(message_id) = message_id
                        && relay_phase != Some("routed")
                    {
                        self.pending_delivery_ids.remove(message_id);
                    }
                    if let Some(message_id) = message_id
                        && matches!(relay_outcome, Some("success" | "timeout" | "failed"))
                    {
                        let _ = Self::remember_seen_id(
                            &mut self.terminal_delivery_message_ids,
                            &mut self.terminal_delivery_message_order,
                            message_id,
                        );
                    }
                    let dedupe_key = format!(
                        "{}:{}:{}:{}:{}",
                        event.target_session,
                        message_id.unwrap_or("<unknown>"),
                        relay_phase.unwrap_or("-"),
                        relay_outcome.unwrap_or("<null>"),
                        relay_reason_code.unwrap_or("-"),
                    );
                    if !Self::remember_seen_id(
                        &mut self.seen_delivery_outcome_ids,
                        &mut self.seen_delivery_outcome_order,
                        dedupe_key,
                    ) {
                        continue;
                    }
                    if relay_phase == Some("routed") {
                        self.push_event(format!(
                            "delivery target={} phase=routed pending={}",
                            event.target_session,
                            self.pending_deliveries_count()
                        ));
                        continue;
                    }
                    let (outcome, reason_code) =
                        map_stream_outcome(relay_outcome, relay_reason_code);
                    self.push_event(format!(
                        "delivery target={} outcome={} reason_code={} pending={}",
                        event.target_session,
                        outcome,
                        reason_code.unwrap_or("-"),
                        self.pending_deliveries_count()
                    ));
                }
                "permission.snapshot" => {
                    let pending_ids = event
                        .payload
                        .get("permission_request_ids")
                        .and_then(serde_json::Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    self.apply_permission_snapshot(pending_ids);
                    self.push_event(format!(
                        "permission snapshot pending_count={}",
                        self.pending_permissions.len()
                    ));
                }
                "permission.requested" => {
                    let Some(permission_request_id) = event
                        .payload
                        .get("permission_request_id")
                        .and_then(serde_json::Value::as_str)
                        .map(ToString::to_string)
                    else {
                        self.push_event(
                            "permission requested event missing permission_request_id".to_string(),
                        );
                        continue;
                    };

                    let entry = PendingPermissionEntry {
                        permission_request_id: permission_request_id.clone(),
                        message_id: event
                            .payload
                            .get("message_id")
                            .and_then(serde_json::Value::as_str)
                            .map(ToString::to_string),
                        target_session: event
                            .payload
                            .get("target_session")
                            .and_then(serde_json::Value::as_str)
                            .map(ToString::to_string),
                        requested_kind: event
                            .payload
                            .get("requested_kind")
                            .and_then(serde_json::Value::as_str)
                            .map(ToString::to_string),
                        requested_details: event.payload.get("requested_details").cloned(),
                        options: parse_permission_options(&event.payload),
                        enqueued_at: event
                            .payload
                            .get("enqueued_at")
                            .and_then(serde_json::Value::as_str)
                            .map(ToString::to_string),
                    };
                    let existed = self.upsert_pending_permission(entry);
                    self.push_event(format!(
                        "permission requested id={} target={} kind={} pending={}",
                        permission_request_id,
                        event
                            .payload
                            .get("target_session")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("-"),
                        event
                            .payload
                            .get("requested_kind")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("-"),
                        self.pending_permissions.len()
                    ));
                    if existed {
                        self.push_status(
                            None,
                            format!(
                                "permission request replayed id={} (deduped)",
                                permission_request_id
                            ),
                        );
                    } else {
                        self.push_status(
                            None,
                            format!("permission requested id={permission_request_id}"),
                        );
                    }
                }
                "permission.resolved" => {
                    let Some(permission_request_id) = event
                        .payload
                        .get("permission_request_id")
                        .and_then(serde_json::Value::as_str)
                    else {
                        self.push_event(
                            "permission resolved event missing permission_request_id".to_string(),
                        );
                        continue;
                    };
                    let removed = self.remove_pending_permission(permission_request_id);
                    let outcome = event
                        .payload
                        .get("outcome")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("-");
                    let reason_code = event
                        .payload
                        .get("reason_code")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("-");
                    self.push_event(format!(
                        "permission resolved id={} outcome={} reason_code={} pending={}",
                        permission_request_id,
                        outcome,
                        reason_code,
                        self.pending_permissions.len()
                    ));
                    if removed {
                        self.push_status(
                            None,
                            format!(
                                "permission resolved id={} outcome={}",
                                permission_request_id, outcome
                            ),
                        );
                    }
                }
                _ => self.push_event(format!(
                    "stream event type={} target={}",
                    event.event_type, event.target_session
                )),
            }
        }
    }

    fn apply_permission_snapshot(&mut self, permission_request_ids: Vec<String>) {
        let mut pending = Vec::<PendingPermissionEntry>::new();
        for permission_request_id in permission_request_ids {
            if let Some(existing) = self
                .pending_permissions
                .iter()
                .find(|entry| entry.permission_request_id == permission_request_id)
                .cloned()
            {
                pending.push(existing);
                continue;
            }
            pending.push(PendingPermissionEntry {
                permission_request_id,
                message_id: None,
                target_session: None,
                requested_kind: None,
                requested_details: None,
                options: Vec::new(),
                enqueued_at: None,
            });
        }
        pending.sort_by(|left, right| {
            match (&left.enqueued_at, &right.enqueued_at) {
                (Some(left_time), Some(right_time)) => left_time.cmp(right_time),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then(left.permission_request_id.cmp(&right.permission_request_id))
        });
        self.pending_permissions = pending;
        self.ensure_pending_permission_selection();
    }

    fn upsert_pending_permission(&mut self, entry: PendingPermissionEntry) -> bool {
        let existing_index = self
            .pending_permissions
            .iter()
            .position(|value| value.permission_request_id == entry.permission_request_id);
        let existed = existing_index.is_some();
        if let Some(index) = existing_index {
            self.pending_permissions[index] = entry;
        } else {
            self.pending_permissions.push(entry);
        }
        self.pending_permissions.sort_by(|left, right| {
            match (&left.enqueued_at, &right.enqueued_at) {
                (Some(left_time), Some(right_time)) => left_time.cmp(right_time),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then(left.permission_request_id.cmp(&right.permission_request_id))
        });
        self.ensure_pending_permission_selection();
        existed
    }

    fn remove_pending_permission(&mut self, permission_request_id: &str) -> bool {
        let Some(index) = self
            .pending_permissions
            .iter()
            .position(|entry| entry.permission_request_id == permission_request_id)
        else {
            return false;
        };
        self.pending_permissions.remove(index);
        self.ensure_pending_permission_selection();
        true
    }
}

fn render_chat_status(status: &ChatStatus) -> &'static str {
    match status {
        ChatStatus::Accepted => "accepted",
        ChatStatus::Success => "success",
        ChatStatus::Partial => "partial",
        ChatStatus::Failure => "failure",
    }
}

fn map_chat_result_outcome(outcome: &ChatOutcome) -> (&'static str, Option<&'static str>) {
    match outcome {
        ChatOutcome::Queued => ("accepted", None),
        ChatOutcome::Delivered => ("success", None),
        ChatOutcome::Timeout => ("timeout", None),
        ChatOutcome::DroppedOnShutdown => ("failed", Some("dropped_on_shutdown")),
        ChatOutcome::Failed => ("failed", None),
    }
}

fn map_stream_outcome<'a>(
    outcome: Option<&'a str>,
    reason_code: Option<&'a str>,
) -> (&'a str, Option<&'a str>) {
    let outcome = outcome.unwrap_or("<unknown>");
    match outcome {
        "success" | "timeout" | "failed" => (outcome, reason_code),
        _ => ("<unknown>", reason_code),
    }
}

fn parse_permission_options(payload: &serde_json::Value) -> Vec<PendingPermissionOption> {
    payload
        .get("requested_details")
        .and_then(|value| value.get("options"))
        .and_then(serde_json::Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    // Relay event payloads serialize PermissionOption (snake_case
                    // Rust struct) into requested_details.options; raw upstream
                    // ACP camelCase lives separately under requested_details.raw.
                    let option_id = option
                        .get("option_id")
                        .and_then(serde_json::Value::as_str)?
                        .to_string();
                    Some(PendingPermissionOption {
                        option_id,
                        name: option
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .map(ToString::to_string),
                        kind: option
                            .get("kind")
                            .and_then(serde_json::Value::as_str)
                            .map(ToString::to_string),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}
