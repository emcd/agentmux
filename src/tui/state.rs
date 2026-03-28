use std::{
    collections::{HashSet, VecDeque},
    io,
    path::PathBuf,
};

use ratatui::widgets::ListState;

use crate::{
    relay::{
        ChatDeliveryMode, ChatOutcome, ChatResult, ChatStatus, Recipient, RelayError, RelayRequest,
        RelayResponse, RelayStreamClientClass, RelayStreamEvent, RelayStreamSession,
    },
    runtime::error::RuntimeError,
};

use super::target::{
    ToCompletionState, append_recipient_token, current_recipient_token_context,
    matching_recipient_candidates, merge_tui_targets,
};

const STATUS_HISTORY_MAXIMUM: usize = 6;
const EVENT_HISTORY_MAXIMUM: usize = 64;
const CHAT_HISTORY_MAXIMUM: usize = 256;
const SEEN_STREAM_IDS_MAXIMUM: usize = 1024;

#[derive(Clone, Debug)]
pub(crate) enum ChatHistoryDirection {
    Outgoing,
    Incoming,
}

#[derive(Clone, Debug)]
pub(crate) struct ChatHistoryEntry {
    pub direction: ChatHistoryDirection,
    pub peer_session: String,
    pub body: String,
    pub message_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TuiLaunchOptions {
    pub bundle_name: String,
    pub sender_session: String,
    pub relay_socket: PathBuf,
    pub look_lines: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FocusField {
    #[default]
    To,
    Message,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusEntry {
    pub code: Option<String>,
    pub message: String,
}

#[derive(Debug)]
pub(crate) struct AppState {
    pub bundle_name: String,
    pub sender_session: String,
    relay_socket: PathBuf,
    relay_stream: RelayStreamSession,
    look_lines: Option<u64>,
    pub recipients: Vec<Recipient>,
    pub recipients_state: ListState,
    pub picker_open: bool,
    pub events_overlay_open: bool,
    pub look_overlay_open: bool,
    look_overlay_restore_picker_on_close: bool,
    pub help_overlay_open: bool,
    pub picker_state: ListState,
    pub focus: FocusField,
    pub to_field: String,
    pub message_field: String,
    message_cursor_index: usize,
    message_cursor_preferred_column: Option<usize>,
    pub look_target: Option<String>,
    pub look_captured_at: Option<String>,
    pub look_snapshot_lines: Vec<String>,
    pub status_history: VecDeque<StatusEntry>,
    pub event_history: VecDeque<String>,
    pub chat_history: VecDeque<ChatHistoryEntry>,
    chat_history_scroll: usize,
    chat_history_viewport_height: usize,
    pending_delivery_ids: HashSet<String>,
    seen_incoming_message_ids: HashSet<String>,
    seen_incoming_message_order: VecDeque<String>,
    seen_delivery_outcome_ids: HashSet<String>,
    seen_delivery_outcome_order: VecDeque<String>,
    relay_stream_poll_error_reported: bool,
    to_completion: Option<ToCompletionState>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(options: TuiLaunchOptions) -> Self {
        let TuiLaunchOptions {
            bundle_name,
            sender_session,
            relay_socket,
            look_lines,
        } = options;
        let relay_stream = RelayStreamSession::new(
            relay_socket.clone(),
            bundle_name.clone(),
            sender_session.clone(),
            RelayStreamClientClass::Ui,
        );
        Self {
            bundle_name,
            sender_session,
            relay_socket,
            relay_stream,
            look_lines,
            recipients: Vec::new(),
            recipients_state: ListState::default(),
            picker_open: false,
            events_overlay_open: false,
            look_overlay_open: false,
            look_overlay_restore_picker_on_close: false,
            help_overlay_open: false,
            picker_state: ListState::default(),
            focus: FocusField::To,
            to_field: String::new(),
            message_field: String::new(),
            message_cursor_index: 0,
            message_cursor_preferred_column: None,
            look_target: None,
            look_captured_at: None,
            look_snapshot_lines: Vec::new(),
            status_history: VecDeque::from([StatusEntry {
                code: None,
                message: "Ready. Press F1 for help.".to_string(),
            }]),
            event_history: VecDeque::new(),
            chat_history: VecDeque::new(),
            chat_history_scroll: 0,
            chat_history_viewport_height: 10,
            pending_delivery_ids: HashSet::new(),
            seen_incoming_message_ids: HashSet::new(),
            seen_incoming_message_order: VecDeque::new(),
            seen_delivery_outcome_ids: HashSet::new(),
            seen_delivery_outcome_order: VecDeque::new(),
            relay_stream_poll_error_reported: false,
            to_completion: None,
            should_quit: false,
        }
    }

    pub fn push_status(&mut self, code: Option<String>, message: impl Into<String>) {
        self.status_history.push_front(StatusEntry {
            code,
            message: message.into(),
        });
        while self.status_history.len() > STATUS_HISTORY_MAXIMUM {
            self.status_history.pop_back();
        }
    }

    pub fn push_runtime_error(&mut self, error: RuntimeError) {
        match error {
            RuntimeError::Validation { code, message } => {
                self.push_status(Some(code), message);
            }
            RuntimeError::InvalidArgument { argument, message } => {
                self.push_status(
                    Some("validation_invalid_arguments".to_string()),
                    format!("invalid argument {argument}: {message}"),
                );
            }
            other => {
                self.push_status(None, other.to_string());
            }
        }
    }

    pub fn refresh_recipients(&mut self) -> Result<(), RuntimeError> {
        let response = self.request_relay(&RelayRequest::List {
            sender_session: Some(self.sender_session.clone()),
        })?;
        match response {
            RelayResponse::List { recipients, .. } => {
                self.recipients = recipients;
                if self.recipients.is_empty() {
                    self.recipients_state.select(None);
                    self.picker_state.select(None);
                } else {
                    self.ensure_recipient_selection();
                }
                self.push_status(
                    None,
                    format!(
                        "Loaded {} recipients for bundle {}.",
                        self.recipients.len(),
                        self.bundle_name
                    ),
                );
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

    pub fn move_picker_selection(&mut self, delta: isize) {
        if self.recipients.is_empty() {
            self.picker_state.select(None);
            return;
        }
        let current = self.picker_state.selected().unwrap_or(0);
        let next = wrap_index(current, delta, self.recipients.len());
        self.picker_state.select(Some(next));
    }

    pub fn open_picker(&mut self) {
        self.picker_open = true;
        self.events_overlay_open = false;
        self.look_overlay_open = false;
        self.look_overlay_restore_picker_on_close = false;
        self.help_overlay_open = false;
        if self.recipients.is_empty() {
            self.picker_state.select(None);
            return;
        }
        let selected = self.recipients_state.selected().unwrap_or(0);
        self.picker_state.select(Some(selected));
    }

    pub fn close_picker(&mut self) {
        self.picker_open = false;
    }

    pub fn toggle_events_overlay(&mut self) {
        self.events_overlay_open = !self.events_overlay_open;
        if self.events_overlay_open {
            self.picker_open = false;
            self.look_overlay_open = false;
            self.look_overlay_restore_picker_on_close = false;
            self.help_overlay_open = false;
        }
    }

    pub fn open_look_overlay(&mut self) {
        self.look_overlay_restore_picker_on_close = self.picker_open;
        self.look_overlay_open = true;
        self.picker_open = false;
        self.events_overlay_open = false;
        self.help_overlay_open = false;
    }

    pub fn close_look_overlay(&mut self) {
        self.look_overlay_open = false;
        let restore_picker = self.look_overlay_restore_picker_on_close;
        self.look_overlay_restore_picker_on_close = false;
        if restore_picker {
            self.open_picker();
        }
    }

    pub fn toggle_help_overlay(&mut self) {
        self.help_overlay_open = !self.help_overlay_open;
        if self.help_overlay_open {
            self.picker_open = false;
            self.events_overlay_open = false;
            self.look_overlay_open = false;
            self.look_overlay_restore_picker_on_close = false;
        }
    }

    pub fn insert_picker_selection(&mut self) {
        let Some(index) = self.picker_state.selected() else {
            self.push_status(
                Some("validation_unknown_target".to_string()),
                "picker has no selected recipient",
            );
            return;
        };
        let Some(recipient) = self.recipients.get(index) else {
            self.push_status(
                Some("validation_unknown_target".to_string()),
                "picker selection is out of range",
            );
            return;
        };
        let session_name = recipient.session_name.clone();
        match self.focus {
            FocusField::To => {
                self.to_field = append_recipient_token(&self.to_field, session_name.as_str())
            }
            FocusField::Message => {
                self.push_status(
                    Some("validation_invalid_arguments".to_string()),
                    "picker inserts recipients only in To field",
                );
                return;
            }
        }
        self.recipients_state.select(Some(index));
        self.picker_open = false;
        self.push_status(None, format!("Inserted recipient {session_name}."));
    }

    pub fn cycle_focus_forward(&mut self) {
        self.focus = match self.focus {
            FocusField::To => FocusField::Message,
            FocusField::Message => FocusField::To,
        };
        self.clear_to_completion();
        self.message_cursor_preferred_column = None;
    }

    pub fn cycle_focus_backward(&mut self) {
        self.focus = match self.focus {
            FocusField::To => FocusField::Message,
            FocusField::Message => FocusField::To,
        };
        self.clear_to_completion();
        self.message_cursor_preferred_column = None;
    }

    pub fn insert_character(&mut self, character: char) {
        match self.focus {
            FocusField::To => {
                self.to_field.push(character);
                self.on_to_field_edited();
                self.maybe_autocomplete_at_prefixed_token();
            }
            FocusField::Message => self.insert_character_in_message(character),
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        for character in text.chars() {
            self.insert_character(character);
        }
    }

    pub fn backspace(&mut self) {
        match self.focus {
            FocusField::To => {
                self.to_field.pop();
                self.on_to_field_edited();
                self.maybe_autocomplete_at_prefixed_token();
            }
            FocusField::Message => {
                self.backspace_message();
            }
        }
    }

    pub fn insert_newline_if_message(&mut self) {
        if self.focus == FocusField::Message {
            self.insert_character_in_message('\n');
        }
    }

    pub fn autocomplete_active_recipient_field(&mut self) {
        if self.focus != FocusField::To {
            return;
        }
        let _ = self.start_to_completion();
    }

    pub fn accept_active_to_completion(&mut self) -> bool {
        if self.focus != FocusField::To {
            return false;
        }
        if self.to_completion.is_none() {
            return false;
        }
        self.to_completion = None;
        true
    }

    pub fn move_to_completion_selection(&mut self, delta: isize) -> bool {
        if self.focus != FocusField::To {
            return false;
        }
        if let Some((token_start, leading_ws, candidate)) =
            self.to_completion.as_mut().and_then(|completion_state| {
                if completion_state.candidates.is_empty() {
                    return None;
                }
                completion_state.candidate_index = wrap_index(
                    completion_state.candidate_index,
                    delta,
                    completion_state.candidates.len(),
                );
                Some((
                    completion_state.token_start,
                    completion_state.leading_ws,
                    completion_state
                        .candidates
                        .get(completion_state.candidate_index)
                        .cloned()
                        .unwrap_or_default(),
                ))
            })
        {
            self.apply_to_completion_candidate(token_start, leading_ws, candidate.as_str());
            return true;
        }
        false
    }

    pub fn move_message_cursor_up(&mut self) {
        if self.focus != FocusField::Message {
            return;
        }
        self.move_message_cursor_vertical(-1);
    }

    pub fn move_message_cursor_down(&mut self) {
        if self.focus != FocusField::Message {
            return;
        }
        self.move_message_cursor_vertical(1);
    }

    pub fn message_cursor_line_and_column(&self) -> (usize, usize) {
        line_and_column_for_index(self.message_field.as_str(), self.message_cursor_index)
    }

    fn start_to_completion(&mut self) -> bool {
        let context = current_recipient_token_context(&self.to_field);
        let Some(context) = context else {
            return false;
        };
        if context.query.is_empty() {
            return false;
        }

        let candidates = self
            .recipients
            .iter()
            .map(|recipient| recipient.session_name.clone())
            .collect::<Vec<_>>();
        let matched = matching_recipient_candidates(&context.query, &candidates);
        if matched.is_empty() {
            return false;
        }

        let candidate = matched.first().cloned().unwrap_or_default();
        self.apply_to_completion_candidate(context.token_start, context.leading_ws, &candidate);
        self.to_completion = Some(ToCompletionState {
            token_start: context.token_start,
            leading_ws: context.leading_ws,
            candidates: matched,
            candidate_index: 0,
        });
        true
    }

    fn on_to_field_edited(&mut self) {
        self.to_completion = None;
    }

    fn maybe_autocomplete_at_prefixed_token(&mut self) {
        if self.focus != FocusField::To {
            return;
        }
        let Some(context) = current_recipient_token_context(&self.to_field) else {
            return;
        };
        if !context.at_prefixed || context.query.is_empty() {
            return;
        }

        let candidates = self
            .recipients
            .iter()
            .map(|recipient| recipient.session_name.clone())
            .collect::<Vec<_>>();
        let matched = matching_recipient_candidates(&context.query, &candidates);
        if matched.is_empty() {
            return;
        }
        let candidate = matched.first().cloned().unwrap_or_default();
        self.apply_to_completion_candidate(context.token_start, context.leading_ws, &candidate);
        self.to_completion = Some(ToCompletionState {
            token_start: context.token_start,
            leading_ws: context.leading_ws,
            candidates: matched,
            candidate_index: 0,
        });
    }

    fn apply_to_completion_candidate(
        &mut self,
        token_start: usize,
        leading_ws: usize,
        candidate: &str,
    ) {
        let token_slice = &self.to_field[token_start..];
        let raw_token = token_slice
            .split(',')
            .next()
            .map(str::trim_end)
            .unwrap_or(token_slice);
        let token_end = token_start + raw_token.len();

        let mut next = String::from(&self.to_field[..token_start]);
        next.push_str(&raw_token[..leading_ws.min(raw_token.len())]);
        next.push_str(candidate);
        next.push_str(&self.to_field[token_end..]);
        self.to_field = next;
    }

    fn clear_to_completion(&mut self) {
        self.to_completion = None;
    }

    fn clear_compose_fields(&mut self) {
        self.to_field.clear();
        self.message_field.clear();
        self.message_cursor_index = 0;
        self.message_cursor_preferred_column = None;
        self.clear_to_completion();
    }

    fn insert_character_in_message(&mut self, character: char) {
        self.message_field
            .insert(self.message_cursor_index, character);
        self.message_cursor_index += character.len_utf8();
        self.message_cursor_preferred_column = None;
    }

    fn backspace_message(&mut self) {
        if self.message_cursor_index == 0 {
            return;
        }
        let next_cursor =
            previous_char_boundary(self.message_field.as_str(), self.message_cursor_index);
        self.message_field
            .replace_range(next_cursor..self.message_cursor_index, "");
        self.message_cursor_index = next_cursor;
        self.message_cursor_preferred_column = None;
    }

    fn move_message_cursor_vertical(&mut self, delta: isize) {
        let line_ranges = line_ranges(self.message_field.as_str());
        if line_ranges.is_empty() {
            return;
        }
        let (current_line, current_column) =
            line_and_column_for_index(self.message_field.as_str(), self.message_cursor_index);
        let target_line = if delta.is_negative() {
            current_line.saturating_sub(delta.unsigned_abs())
        } else {
            (current_line + delta as usize).min(line_ranges.len().saturating_sub(1))
        };
        if target_line == current_line {
            return;
        }
        let preferred_column = self
            .message_cursor_preferred_column
            .unwrap_or(current_column);
        self.message_cursor_index = cursor_index_for_line_column(
            self.message_field.as_str(),
            line_ranges[target_line],
            preferred_column,
        );
        self.message_cursor_preferred_column = Some(preferred_column);
    }

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
            delivery_mode: ChatDeliveryMode::Async,
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

    pub fn look_picker_target(&mut self) -> Result<(), RuntimeError> {
        let target = self.selected_picker_recipient_id().ok_or_else(|| {
            RuntimeError::validation(
                "validation_unknown_target",
                "look requires a selected recipient in picker",
            )
        })?;

        let response = self.request_relay(&RelayRequest::Look {
            requester_session: self.sender_session.clone(),
            target_session: target.clone(),
            lines: self.look_lines.map(|value| value as usize),
            bundle_name: None,
        })?;

        match response {
            RelayResponse::Look {
                target_session,
                captured_at,
                snapshot_lines,
                ..
            } => {
                self.look_target = Some(target_session.clone());
                self.look_captured_at = Some(captured_at);
                self.look_snapshot_lines = snapshot_lines;
                self.open_look_overlay();
                self.push_status(None, format!("look captured target={target_session}"));
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

    pub fn visible_chat_history_entries(&self) -> Vec<ChatHistoryEntry> {
        let max_items = self.chat_history_viewport_height.max(1);
        let mut visible = self
            .chat_history
            .iter()
            .skip(self.chat_history_scroll)
            .take(max_items)
            .cloned()
            .collect::<Vec<_>>();
        visible.reverse();
        visible
    }

    pub fn pending_deliveries_count(&self) -> usize {
        self.pending_delivery_ids.len()
    }

    fn selected_picker_recipient_id(&self) -> Option<String> {
        self.picker_state
            .selected()
            .and_then(|index| self.recipients.get(index))
            .map(|recipient| recipient.session_name.clone())
    }

    pub fn poll_relay_events(&mut self) {
        match self.relay_stream.poll_events() {
            Ok(events) => {
                if self.relay_stream_poll_error_reported {
                    self.push_status(None, "relay stream reconnected");
                }
                self.relay_stream_poll_error_reported = false;
                self.record_stream_events(&events);
            }
            Err(source) => {
                if self.relay_stream_poll_error_reported {
                    return;
                }
                self.relay_stream_poll_error_reported = true;
                self.push_runtime_error(map_relay_request_failure(&self.relay_socket, source));
            }
        }
    }

    fn ensure_recipient_selection(&mut self) {
        if self.recipients.is_empty() {
            self.recipients_state.select(None);
            self.picker_state.select(None);
            return;
        }
        let index = self
            .recipients_state
            .selected()
            .filter(|index| *index < self.recipients.len())
            .unwrap_or(0);
        self.recipients_state.select(Some(index));
        self.picker_state.select(Some(index));
    }

    fn request_relay(&mut self, request: &RelayRequest) -> Result<RelayResponse, RuntimeError> {
        match self.relay_stream.request_with_events(request) {
            Ok((response, events)) => {
                self.record_stream_events(&events);
                Ok(response)
            }
            Err(source) => Err(map_relay_request_failure(&self.relay_socket, source)),
        }
    }

    fn push_event(&mut self, event: impl Into<String>) {
        self.event_history.push_front(event.into());
        while self.event_history.len() > EVENT_HISTORY_MAXIMUM {
            self.event_history.pop_back();
        }
    }

    fn push_chat_history_entry(&mut self, entry: ChatHistoryEntry) {
        self.chat_history.push_front(entry);
        while self.chat_history.len() > CHAT_HISTORY_MAXIMUM {
            self.chat_history.pop_back();
        }
        self.chat_history_scroll = 0;
    }

    fn push_outgoing_chat_history(&mut self, targets: &[String], body: &str) {
        let peer_session = targets.join(", ");
        self.push_chat_history_entry(ChatHistoryEntry {
            direction: ChatHistoryDirection::Outgoing,
            peer_session,
            body: body.trim_end_matches('\n').to_string(),
            message_id: None,
        });
    }

    fn push_incoming_chat_history(
        &mut self,
        sender_session: &str,
        body: &str,
        message_id: Option<&str>,
    ) {
        self.push_chat_history_entry(ChatHistoryEntry {
            direction: ChatHistoryDirection::Incoming,
            peer_session: sender_session.to_string(),
            body: body.to_string(),
            message_id: message_id.map(ToString::to_string),
        });
    }

    fn max_chat_history_scroll(&self) -> usize {
        let visible = self.chat_history_viewport_height.max(1);
        self.chat_history.len().saturating_sub(visible)
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

    fn record_chat_events(&mut self, status: &ChatStatus, results: &[ChatResult]) {
        let mut accepted_count = 0usize;
        for result in results {
            match result.outcome {
                ChatOutcome::Queued => {
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

    fn record_stream_events(&mut self, events: &[RelayStreamEvent]) {
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
                    self.push_incoming_chat_history(sender_session, body, Some(message_id));
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
                    let relay_outcome = event
                        .payload
                        .get("outcome")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("<unknown>");
                    let relay_reason_code = event
                        .payload
                        .get("reason_code")
                        .and_then(serde_json::Value::as_str);
                    if let Some(message_id) = message_id {
                        self.pending_delivery_ids.remove(message_id);
                    }
                    let dedupe_key = format!(
                        "{}:{}:{}:{}",
                        event.target_session,
                        message_id.unwrap_or("<unknown>"),
                        relay_outcome,
                        relay_reason_code.unwrap_or("-"),
                    );
                    if !Self::remember_seen_id(
                        &mut self.seen_delivery_outcome_ids,
                        &mut self.seen_delivery_outcome_order,
                        dedupe_key,
                    ) {
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
                _ => self.push_event(format!(
                    "stream event type={} target={}",
                    event.event_type, event.target_session
                )),
            }
        }
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
    outcome: &'a str,
    reason_code: Option<&'a str>,
) -> (&'a str, Option<&'a str>) {
    match outcome {
        "success" | "timeout" | "failed" => (outcome, reason_code),
        _ => ("<unknown>", reason_code),
    }
}

fn wrap_index(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    ((index as isize + delta).rem_euclid(len)) as usize
}

fn previous_char_boundary(value: &str, cursor_index: usize) -> usize {
    value[..cursor_index]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn line_ranges(value: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::<(usize, usize)>::new();
    let mut line_start = 0usize;
    for (index, character) in value.char_indices() {
        if character == '\n' {
            ranges.push((line_start, index));
            line_start = index + character.len_utf8();
        }
    }
    ranges.push((line_start, value.len()));
    ranges
}

fn line_and_column_for_index(value: &str, cursor_index: usize) -> (usize, usize) {
    let ranges = line_ranges(value);
    for (line_index, (line_start, line_end)) in ranges.iter().enumerate() {
        if cursor_index <= *line_end || line_index + 1 == ranges.len() {
            let column_end = cursor_index.min(*line_end);
            let column = value[*line_start..column_end].chars().count();
            return (line_index, column);
        }
    }
    (0, 0)
}

fn cursor_index_for_line_column(
    value: &str,
    line_range: (usize, usize),
    target_column: usize,
) -> usize {
    let (line_start, line_end) = line_range;
    let line_slice = &value[line_start..line_end];
    let line_len = line_slice.chars().count();
    let clamped_column = target_column.min(line_len);
    if clamped_column == line_len {
        return line_end;
    }
    line_start
        + line_slice
            .char_indices()
            .nth(clamped_column)
            .map(|(index, _)| index)
            .unwrap_or(0)
}

fn map_relay_error(error: RelayError) -> RuntimeError {
    if error.code.starts_with("validation_") || error.code == "relay_unavailable" {
        return RuntimeError::validation(error.code, error.message);
    }
    RuntimeError::io(
        error.message,
        io::Error::other("relay returned internal error"),
    )
}

fn map_relay_request_failure(socket_path: &std::path::Path, source: io::Error) -> RuntimeError {
    if is_relay_unavailable_error(&source) {
        return RuntimeError::validation(
            "relay_unavailable",
            format!(
                "relay is unavailable at {}; start agentmux host relay with matching state-directory",
                socket_path.display()
            ),
        );
    }
    RuntimeError::io(
        format!("relay request failed for {}", socket_path.display()),
        source,
    )
}

fn is_relay_unavailable_error(source: &io::Error) -> bool {
    matches!(
        source.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        AppState, ChatHistoryDirection, ChatHistoryEntry, RelayStreamEvent, TuiLaunchOptions,
    };

    fn make_state() -> AppState {
        AppState::new(TuiLaunchOptions {
            bundle_name: "agentmux".to_string(),
            sender_session: "tui".to_string(),
            relay_socket: PathBuf::from("/tmp/agentmux-test-relay.sock"),
            look_lines: None,
        })
    }

    #[test]
    fn chat_history_viewport_pages_oldest_to_newest() {
        let mut state = make_state();
        for index in 0..6 {
            state.push_chat_history_entry(ChatHistoryEntry {
                direction: ChatHistoryDirection::Outgoing,
                peer_session: "relay".to_string(),
                body: format!("message-{index}"),
                message_id: None,
            });
        }

        state.set_chat_history_viewport_height(3);
        let first_page = state.visible_chat_history_entries();
        let first_bodies = first_page
            .iter()
            .map(|entry| entry.body.clone())
            .collect::<Vec<_>>();
        assert_eq!(first_bodies, vec!["message-3", "message-4", "message-5"]);

        state.scroll_chat_history_page_up();
        let second_page = state.visible_chat_history_entries();
        let second_bodies = second_page
            .iter()
            .map(|entry| entry.body.clone())
            .collect::<Vec<_>>();
        assert_eq!(second_bodies, vec!["message-0", "message-1", "message-2"]);
    }

    #[test]
    fn record_stream_events_deduplicates_incoming_message_ids() {
        let mut state = make_state();
        let duplicated = RelayStreamEvent {
            event_type: "incoming_message".to_string(),
            bundle_name: "agentmux".to_string(),
            target_session: "tui".to_string(),
            created_at: "2026-03-19T00:00:00Z".to_string(),
            payload: json!({
                "message_id": "msg-1",
                "sender_session": "relay",
                "body": "hello"
            }),
        };

        state.record_stream_events(&[duplicated.clone(), duplicated]);
        assert_eq!(state.chat_history.len(), 1);
        assert_eq!(state.event_history.len(), 1);
        assert_eq!(
            state.chat_history.front().map(|entry| entry.body.as_str()),
            Some("hello")
        );
    }

    #[test]
    fn closing_look_overlay_restores_picker_when_opened_from_picker() {
        let mut state = make_state();
        state.recipients = vec![crate::relay::Recipient {
            session_name: "master".to_string(),
            display_name: None,
        }];
        state.recipients_state.select(Some(0));
        state.open_picker();
        assert!(state.picker_open);
        state.open_look_overlay();
        assert!(state.look_overlay_open);
        assert!(!state.picker_open);

        state.close_look_overlay();
        assert!(!state.look_overlay_open);
        assert!(state.picker_open);
    }

    #[test]
    fn closing_look_overlay_without_picker_context_does_not_open_picker() {
        let mut state = make_state();
        assert!(!state.picker_open);
        state.open_look_overlay();
        assert!(state.look_overlay_open);
        assert!(!state.picker_open);

        state.close_look_overlay();
        assert!(!state.look_overlay_open);
        assert!(!state.picker_open);
    }
}
