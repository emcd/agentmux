use std::{
    collections::{HashSet, VecDeque},
    io,
    path::PathBuf,
};

use ratatui::widgets::ListState;

use crate::{
    relay::{
        ChatDeliveryMode, ChatStatus, Recipient, RelayError, RelayRequest, RelayResponse,
        request_relay,
    },
    runtime::error::RuntimeError,
};

const MAX_STATUS_HISTORY: usize = 6;

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
    Cc,
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
    look_lines: Option<u64>,
    pub recipients: Vec<Recipient>,
    pub recipients_state: ListState,
    pub picker_open: bool,
    pub picker_state: ListState,
    pub focus: FocusField,
    pub to_field: String,
    pub cc_field: String,
    pub message_field: String,
    pub delivery_mode: ChatDeliveryMode,
    pub look_target: Option<String>,
    pub look_captured_at: Option<String>,
    pub look_snapshot_lines: Vec<String>,
    pub status_history: VecDeque<StatusEntry>,
    pub last_delivery_lines: Vec<String>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(options: TuiLaunchOptions) -> Self {
        Self {
            bundle_name: options.bundle_name,
            sender_session: options.sender_session,
            relay_socket: options.relay_socket,
            look_lines: options.look_lines,
            recipients: Vec::new(),
            recipients_state: ListState::default(),
            picker_open: false,
            picker_state: ListState::default(),
            focus: FocusField::To,
            to_field: String::new(),
            cc_field: String::new(),
            message_field: String::new(),
            delivery_mode: ChatDeliveryMode::Async,
            look_target: None,
            look_captured_at: None,
            look_snapshot_lines: Vec::new(),
            status_history: VecDeque::from([StatusEntry {
                code: None,
                message: "Ready. Ctrl+S send, Ctrl+L look, F2 picker, Esc/Ctrl+Q quit.".to_string(),
            }]),
            last_delivery_lines: Vec::new(),
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

    pub fn toggle_delivery_mode(&mut self) {
        self.delivery_mode = match self.delivery_mode {
            ChatDeliveryMode::Async => ChatDeliveryMode::Sync,
            ChatDeliveryMode::Sync => ChatDeliveryMode::Async,
        };
        self.push_status(
            None,
            format!("Delivery mode set to {:?}.", self.delivery_mode),
        );
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
        match self.focus {
            FocusField::To => {
                self.to_field =
                    append_recipient_token(&self.to_field, recipient.session_name.as_str())
            }
            FocusField::Cc => {
                self.cc_field =
                    append_recipient_token(&self.cc_field, recipient.session_name.as_str())
            }
            FocusField::Message => {
                self.push_status(
                    Some("validation_invalid_arguments".to_string()),
                    "picker inserts recipients only in To/Cc fields",
                );
                return;
            }
        }
        self.recipients_state.select(Some(index));
        self.picker_open = false;
        self.push_status(
            None,
            format!("Inserted recipient {}.", recipient.session_name),
        );
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            FocusField::To => FocusField::Cc,
            FocusField::Cc => FocusField::Message,
            FocusField::Message => FocusField::To,
        };
    }

    pub fn insert_character(&mut self, character: char) {
        match self.focus {
            FocusField::To => self.to_field.push(character),
            FocusField::Cc => self.cc_field.push(character),
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
            }
            FocusField::Cc => {
                self.cc_field.pop();
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
        let candidates = self
            .recipients
            .iter()
            .map(|recipient| recipient.session_name.clone())
            .collect::<Vec<_>>();
        match self.focus {
            FocusField::To => {
                if let Some(next) = autocomplete_recipient_input(&self.to_field, &candidates) {
                    self.to_field = next;
                }
            }
            FocusField::Cc => {
                if let Some(next) = autocomplete_recipient_input(&self.cc_field, &candidates) {
                    self.cc_field = next;
                }
            }
            FocusField::Message => {}
        }
    }

    pub fn send_message(&mut self) -> Result<(), RuntimeError> {
        if self.message_field.trim().is_empty() {
            return Err(RuntimeError::validation(
                "validation_missing_message_input",
                "message body is required",
            ));
        }
        let targets = merge_tui_targets(&self.to_field, &self.cc_field, &self.bundle_name)?;
        let response = self.request_relay(&RelayRequest::Chat {
            request_id: None,
            sender_session: self.sender_session.clone(),
            message: self.message_field.clone(),
            targets,
            broadcast: false,
            delivery_mode: self.delivery_mode,
            quiet_window_ms: None,
            quiescence_timeout_ms: None,
        })?;
        match response {
            RelayResponse::Chat {
                delivery_mode,
                status,
                results,
                ..
            } => {
                self.last_delivery_lines = results
                    .into_iter()
                    .map(|result| {
                        let outcome = serde_json::to_value(&result.outcome)
                            .ok()
                            .and_then(|value| value.as_str().map(ToString::to_string))
                            .unwrap_or_else(|| format!("{:?}", result.outcome));
                        if let Some(reason) = result.reason {
                            format!(
                                "target={} outcome={} reason={}",
                                result.target_session, outcome, reason
                            )
                        } else {
                            format!("target={} outcome={}", result.target_session, outcome)
                        }
                    })
                    .collect::<Vec<_>>();
                self.push_status(
                    None,
                    format!(
                        "send completed mode={delivery_mode:?} status={}",
                        render_chat_status(status)
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
                merge_tui_targets(&self.to_field, &self.cc_field, &self.bundle_name)
                    .ok()
                    .and_then(|targets| targets.first().cloned())
            })
            .ok_or_else(|| {
                RuntimeError::validation(
                    "validation_unknown_target",
                    "look requires a selected recipient or To/Cc target",
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
            FocusField::Cc => "Cc",
            FocusField::Message => "Message",
        }
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

    fn request_relay(&self, request: &RelayRequest) -> Result<RelayResponse, RuntimeError> {
        request_relay(&self.relay_socket, request)
            .map_err(|source| map_relay_request_failure(&self.relay_socket, source))
    }
}

fn render_chat_status(status: ChatStatus) -> &'static str {
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
/// - qualified: `<bundle-id>/<session-id>` (same-bundle only in MVP)
pub fn parse_tui_target_identifier(
    identifier: &str,
    associated_bundle: &str,
) -> Result<String, RuntimeError> {
    let trimmed = identifier.trim();
    if trimmed.is_empty() {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            "target identifier must be non-empty",
        ));
    }

    let Some((candidate_bundle, candidate_session)) = trimmed.split_once('/') else {
        return Ok(trimmed.to_string());
    };

    if candidate_bundle.is_empty()
        || candidate_session.is_empty()
        || candidate_session.contains('/')
    {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            format!("target identifier '{trimmed}' is invalid"),
        ));
    }

    if candidate_bundle != associated_bundle {
        return Err(RuntimeError::validation(
            "validation_cross_bundle_unsupported",
            "cross-bundle targets are unsupported in TUI MVP",
        ));
    }

    Ok(candidate_session.to_string())
}

/// Merges To/Cc recipient fields into a deterministic target set.
pub fn merge_tui_targets(
    to_field: &str,
    cc_field: &str,
    associated_bundle: &str,
) -> Result<Vec<String>, RuntimeError> {
    let mut targets = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();

    for field_value in [to_field, cc_field] {
        for token in field_value
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let normalized = parse_tui_target_identifier(token, associated_bundle)?;
            if seen.insert(normalized.clone()) {
                targets.push(normalized);
            }
        }
    }

    if targets.is_empty() {
        return Err(RuntimeError::validation(
            "validation_empty_targets",
            "provide at least one recipient in To or Cc",
        ));
    }

    Ok(targets)
}

/// Completes the current recipient token from a list of candidate identities.
pub fn autocomplete_recipient_input(field_value: &str, candidates: &[String]) -> Option<String> {
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
    let prefix = token_slice[leading_ws..].trim();

    let mut candidate_values = candidates
        .iter()
        .map(|candidate| candidate.as_str())
        .filter(|candidate| prefix.is_empty() || candidate.starts_with(prefix))
        .collect::<Vec<_>>();
    candidate_values.sort_unstable();
    let selected = candidate_values.first().copied()?;

    let mut next = String::from(&field_value[..token_start]);
    next.push_str(&token_slice[..leading_ws]);
    next.push_str(selected);
    Some(next)
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
