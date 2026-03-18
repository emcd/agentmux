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

const MAX_STATUS_HISTORY: usize = 6;
const MAX_EVENT_HISTORY: usize = 64;

#[derive(Clone, Debug)]
struct ToCompletionState {
    token_start: usize,
    leading_ws: usize,
    candidates: Vec<String>,
    candidate_index: usize,
}

#[derive(Clone, Debug)]
struct RecipientTokenContext {
    token_start: usize,
    leading_ws: usize,
    query: String,
    at_prefixed: bool,
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
    pub picker_state: ListState,
    pub focus: FocusField,
    pub to_field: String,
    pub message_field: String,
    pub look_target: Option<String>,
    pub look_captured_at: Option<String>,
    pub look_snapshot_lines: Vec<String>,
    pub status_history: VecDeque<StatusEntry>,
    pub event_history: VecDeque<String>,
    pending_delivery_ids: HashSet<String>,
    to_completion: Option<ToCompletionState>,
    completion_locked_until_to_edit: bool,
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
            picker_state: ListState::default(),
            focus: FocusField::To,
            to_field: String::new(),
            message_field: String::new(),
            look_target: None,
            look_captured_at: None,
            look_snapshot_lines: Vec::new(),
            status_history: VecDeque::from([StatusEntry {
                code: None,
                message: "Ready. Tab complete/focus, Enter accept completion, Ctrl+Space cycle, Ctrl+S send, Ctrl+L look, Esc/Ctrl+Q quit.".to_string(),
            }]),
            event_history: VecDeque::new(),
            pending_delivery_ids: HashSet::new(),
            to_completion: None,
            completion_locked_until_to_edit: false,
            should_quit: false,
        }
    }

    pub fn push_status(&mut self, code: Option<String>, message: impl Into<String>) {
        self.status_history.push_front(StatusEntry {
            code,
            message: message.into(),
        });
        while self.status_history.len() > MAX_STATUS_HISTORY {
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
    }

    pub fn cycle_focus_backward(&mut self) {
        self.focus = match self.focus {
            FocusField::To => FocusField::Message,
            FocusField::Message => FocusField::To,
        };
        self.clear_to_completion();
    }

    pub fn insert_character(&mut self, character: char) {
        match self.focus {
            FocusField::To => {
                self.to_field.push(character);
                self.on_to_field_edited();
                self.maybe_autocomplete_at_prefixed_token();
            }
            FocusField::Message => self.message_field.push(character),
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
                self.message_field.pop();
            }
        }
    }

    pub fn insert_newline_if_message(&mut self) {
        if self.focus == FocusField::Message {
            self.message_field.push('\n');
        }
    }

    pub fn autocomplete_active_recipient_field(&mut self) {
        if self.focus != FocusField::To {
            return;
        }
        let _ = self.cycle_or_start_to_completion();
    }

    pub fn handle_tab_in_to_field(&mut self) -> bool {
        if self.completion_locked_until_to_edit {
            return false;
        }
        self.cycle_or_start_to_completion()
    }

    pub fn accept_active_to_completion(&mut self) -> bool {
        if self.focus != FocusField::To {
            return false;
        }
        if self.to_completion.is_none() {
            return false;
        }
        self.to_completion = None;
        self.completion_locked_until_to_edit = true;
        true
    }

    fn cycle_or_start_to_completion(&mut self) -> bool {
        if self.focus != FocusField::To {
            return false;
        }

        if let Some((token_start, leading_ws, candidate)) =
            self.to_completion.as_mut().and_then(|state| {
                if state.candidates.is_empty() {
                    return None;
                }
                state.candidate_index = (state.candidate_index + 1) % state.candidates.len();
                Some((
                    state.token_start,
                    state.leading_ws,
                    state
                        .candidates
                        .get(state.candidate_index)
                        .cloned()
                        .unwrap_or_default(),
                ))
            })
        {
            self.apply_to_completion_candidate(token_start, leading_ws, &candidate);
            return true;
        }

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
        self.completion_locked_until_to_edit = false;
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

    pub fn send_message(&mut self) -> Result<(), RuntimeError> {
        if self.message_field.trim().is_empty() {
            return Err(RuntimeError::validation(
                "validation_missing_message_input",
                "message body is required",
            ));
        }
        let targets = merge_tui_targets(&self.to_field, &self.bundle_name)?;
        let response = self.request_relay(&RelayRequest::Chat {
            request_id: None,
            sender_session: self.sender_session.clone(),
            message: self.message_field.clone(),
            targets,
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: None,
            quiescence_timeout_ms: None,
        })?;
        match response {
            RelayResponse::Chat {
                status, results, ..
            } => {
                self.record_chat_events(&status, &results);
                self.push_status(
                    None,
                    format!(
                        "send accepted status={} pending={}",
                        render_chat_status(&status),
                        self.pending_deliveries_count()
                    ),
                );
                Ok(())
            }
            RelayResponse::Error { error } => Err(map_relay_error(error)),
            other => Err(RuntimeError::validation(
                "internal_unexpected_failure",
                format!("relay returned unexpected response variant: {other:?}"),
            )),
        }
    }

    pub fn look_target(&mut self) -> Result<(), RuntimeError> {
        let target = self
            .selected_recipient_id()
            .or_else(|| {
                merge_tui_targets(&self.to_field, &self.bundle_name)
                    .ok()
                    .and_then(|targets| targets.first().cloned())
            })
            .ok_or_else(|| {
                RuntimeError::validation(
                    "validation_unknown_target",
                    "look requires a selected recipient or To target",
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
                self.push_status(None, format!("look captured target={target_session}"));
                Ok(())
            }
            RelayResponse::Error { error } => Err(map_relay_error(error)),
            other => Err(RuntimeError::validation(
                "internal_unexpected_failure",
                format!("relay returned unexpected response variant: {other:?}"),
            )),
        }
    }

    pub fn active_recipient_field_name(&self) -> &'static str {
        match self.focus {
            FocusField::To => "To",
            FocusField::Message => "Message",
        }
    }

    pub fn pending_deliveries_count(&self) -> usize {
        self.pending_delivery_ids.len()
    }

    pub fn selected_recipient_id(&self) -> Option<String> {
        self.recipients_state
            .selected()
            .and_then(|index| self.recipients.get(index))
            .map(|recipient| recipient.session_name.clone())
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
        while self.event_history.len() > MAX_EVENT_HISTORY {
            self.event_history.pop_back();
        }
    }

    fn record_chat_events(&mut self, status: &ChatStatus, results: &[ChatResult]) {
        let mut queued_count = 0usize;
        for result in results {
            match result.outcome {
                ChatOutcome::Queued => {
                    self.pending_delivery_ids.insert(result.message_id.clone());
                    queued_count += 1;
                }
                _ => {
                    self.pending_delivery_ids.remove(&result.message_id);
                }
            }
        }

        self.push_event(format!(
            "send status={} targets={} queued={} pending={}",
            render_chat_status(status),
            results.len(),
            queued_count,
            self.pending_deliveries_count()
        ));

        for result in results {
            let outcome = serde_json::to_value(&result.outcome)
                .ok()
                .and_then(|value| value.as_str().map(ToString::to_string))
                .unwrap_or_else(|| format!("{:?}", result.outcome));
            if let Some(reason) = result.reason.as_deref() {
                self.push_event(format!(
                    "target={} outcome={} message_id={} reason={}",
                    result.target_session, outcome, result.message_id, reason
                ));
            } else {
                self.push_event(format!(
                    "target={} outcome={} message_id={}",
                    result.target_session, outcome, result.message_id
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
                    if let Some(message_id) = message_id {
                        self.pending_delivery_ids.remove(message_id);
                    }
                    let outcome = event
                        .payload
                        .get("outcome")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("<unknown>");
                    let reason_code = event
                        .payload
                        .get("reason_code")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("-");
                    self.push_event(format!(
                        "delivery target={} outcome={} reason_code={} pending={}",
                        event.target_session,
                        outcome,
                        reason_code,
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

fn wrap_index(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    ((index as isize + delta).rem_euclid(len)) as usize
}

/// Parses one recipient identifier for TUI send/look workflows.
///
/// Accepted forms:
/// - local: `<session-id>`
pub fn parse_tui_target_identifier(
    identifier: &str,
    _associated_bundle: &str,
) -> Result<String, RuntimeError> {
    let trimmed = identifier.trim().trim_start_matches('@');
    if trimmed.is_empty() {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            "target identifier must be non-empty",
        ));
    }
    if trimmed.contains('/') {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            format!("target identifier '{trimmed}' is invalid; use session id only"),
        ));
    }
    Ok(trimmed.to_string())
}

/// Merges the To recipient field into a deterministic target set.
pub fn merge_tui_targets(
    to_field: &str,
    associated_bundle: &str,
) -> Result<Vec<String>, RuntimeError> {
    let mut targets = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();

    for token in to_field
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let normalized = parse_tui_target_identifier(token, associated_bundle)?;
        if seen.insert(normalized.clone()) {
            targets.push(normalized);
        }
    }

    if targets.is_empty() {
        return Err(RuntimeError::validation(
            "validation_empty_targets",
            "provide at least one recipient in To",
        ));
    }

    Ok(targets)
}

/// Completes the current recipient token from a list of candidate identities.
pub fn autocomplete_recipient_input(field_value: &str, candidates: &[String]) -> Option<String> {
    let context = current_recipient_token_context(field_value)?;
    let selected = matching_recipient_candidates(&context.query, candidates)
        .first()
        .cloned()?;

    let mut next = String::from(&field_value[..context.token_start]);
    let token_slice = &field_value[context.token_start..];
    next.push_str(&token_slice[..context.leading_ws]);
    next.push_str(selected.as_str());
    Some(next)
}

fn matching_recipient_candidates(query: &str, candidates: &[String]) -> Vec<String> {
    let mut matched = candidates
        .iter()
        .filter(|candidate| query.is_empty() || candidate.starts_with(query))
        .cloned()
        .collect::<Vec<_>>();
    matched.sort_unstable();
    matched
}

fn current_recipient_token_context(field_value: &str) -> Option<RecipientTokenContext> {
    let token_start = field_value.rfind(',').map(|index| index + 1).unwrap_or(0);
    let token_slice = &field_value[token_start..];
    let leading_ws = token_slice
        .char_indices()
        .find_map(|(index, character)| {
            if character.is_whitespace() {
                None
            } else {
                Some(index)
            }
        })
        .unwrap_or(token_slice.len());
    let token_text = token_slice[leading_ws..].trim().to_string();
    if token_text.is_empty() {
        return None;
    }

    let (at_prefixed, query) = if let Some(rest) = token_text.strip_prefix('@') {
        (true, rest.to_string())
    } else {
        (false, token_text.clone())
    };

    Some(RecipientTokenContext {
        token_start,
        leading_ws,
        query,
        at_prefixed,
    })
}

pub(crate) fn append_recipient_token(field_value: &str, recipient: &str) -> String {
    let mut tokens = field_value
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if tokens.iter().any(|token| token == recipient) {
        return field_value.to_string();
    }
    tokens.push(recipient.to_string());
    tokens.join(", ")
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
                "relay is unavailable at {}; start host relay with matching bundle and state-directory",
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
