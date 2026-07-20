use std::{
    collections::{HashSet, VecDeque},
    io,
    path::PathBuf,
};

use ratatui::widgets::ListState;

use crate::{
    relay::{RelayError, RelayStreamSession},
    runtime::error::RuntimeError,
    transports::StructuredEntry,
};

use super::target::{
    ToCompletionState, append_recipient_token, current_recipient_token_context,
    matching_recipient_candidates, merge_tui_targets, sender_bound_bundle,
};

mod compose;
mod history;
mod relay;

use super::status::BundleStatusDisplay;

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LookSnapshotFormat {
    Lines,
    StructuredEntriesV1,
}

#[derive(Clone, Debug)]
pub struct TuiLaunchOptions {
    pub namespace: String,
    pub sender_session: String,
    pub relay_socket: PathBuf,
    pub look_lines: Option<u64>,
    pub available_bundles: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FocusField {
    #[default]
    To,
    Message,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ScreenMode {
    #[default]
    Communication,
    Interaction,
}

/// Which column of the unified bundle+session picker currently has keyboard
/// focus. The filter and Up/Down navigation apply to the focused column; the
/// other column shows its full (unfiltered) list.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PickerColumn {
    Bundles,
    #[default]
    Sessions,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusEntry {
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Recipient {
    pub session_name: String,
    pub display_name: Option<String>,
    pub ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingChoiceEntry {
    pub choice_request_id: String,
    pub message_id: Option<String>,
    pub target_session: Option<String>,
    pub requested_kind: Option<String>,
    pub requested_details: Option<serde_json::Value>,
    pub enqueued_at: Option<String>,
    pub options: Vec<PendingChoiceOption>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingChoiceOption {
    pub option_id: String,
    pub name: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug)]
pub(crate) struct AppState {
    pub namespace: String,
    pub sender_session: String,
    relay_socket: PathBuf,
    relay_stream: RelayStreamSession,
    look_lines: Option<u64>,
    pub recipients: Vec<Recipient>,
    /// Relay-wide To-field completion candidates as full `session@bundle`
    /// principal ids, spanning every available bundle other than the active
    /// `namespace` (whose sessions are already offered via `recipients`, which
    /// the relay returns as canonical `session@bundle` ids too). Refreshed
    /// eagerly on the same cadence as `recipients`; kept separate so the
    /// recipient picker stays scoped to the active bundle.
    pub cross_bundle_candidates: Vec<String>,
    pub bundle_status: Option<BundleStatusDisplay>,
    pub last_selected_recipient: Option<String>,
    pub available_bundles: Vec<String>,
    pub picker_open: bool,
    pub picker_focus: PickerColumn,
    pub picker_filter: String,
    pub events_overlay_open: bool,
    pub help_overlay_open: bool,
    pub picker_session_state: ListState,
    pub picker_bundle_state: ListState,
    pub mode: ScreenMode,
    pub focus: FocusField,
    pub to_field: String,
    to_cursor_index: usize,
    pub message_field: String,
    message_cursor_index: usize,
    message_cursor_preferred_column: Option<usize>,
    pub raww_draft: String,
    pub(crate) raww_cursor_index: usize,
    raww_cursor_preferred_column: Option<usize>,
    pub look_target: Option<String>,
    pub look_captured_at: Option<String>,
    pub look_snapshot_format: Option<LookSnapshotFormat>,
    pub look_snapshot_lines: Vec<String>,
    pub look_snapshot_entries: Vec<StructuredEntry>,
    pub look_overlay_scroll: usize,
    pub(crate) look_choice_request_index: usize,
    pub(crate) look_choice_option_index: usize,
    pub status_history: VecDeque<StatusEntry>,
    pub event_history: VecDeque<String>,
    pub pending_choices: Vec<PendingChoiceEntry>,
    pub pending_choices_state: ListState,
    pub chat_history: VecDeque<ChatHistoryEntry>,
    chat_history_scroll: usize,
    chat_history_viewport_height: usize,
    chat_history_total_lines: usize,
    pending_delivery_ids: HashSet<String>,
    terminal_delivery_message_ids: HashSet<String>,
    terminal_delivery_message_order: VecDeque<String>,
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
            namespace,
            sender_session,
            relay_socket,
            look_lines,
            available_bundles,
        } = options;
        let relay_stream = RelayStreamSession::new(
            relay_socket.clone(),
            namespace.clone(),
            sender_session.clone(),
        );
        Self {
            namespace,
            sender_session,
            relay_socket,
            relay_stream,
            look_lines,
            recipients: Vec::new(),
            cross_bundle_candidates: Vec::new(),
            bundle_status: None,
            last_selected_recipient: None,
            available_bundles,
            picker_open: false,
            picker_focus: PickerColumn::Sessions,
            picker_filter: String::new(),
            events_overlay_open: false,
            help_overlay_open: false,
            picker_session_state: ListState::default(),
            picker_bundle_state: ListState::default(),
            mode: ScreenMode::Communication,
            focus: FocusField::To,
            to_field: String::new(),
            to_cursor_index: 0,
            message_field: String::new(),
            message_cursor_index: 0,
            message_cursor_preferred_column: None,
            raww_draft: String::new(),
            raww_cursor_index: 0,
            raww_cursor_preferred_column: None,
            look_target: None,
            look_captured_at: None,
            look_snapshot_format: None,
            look_snapshot_lines: Vec::new(),
            look_snapshot_entries: Vec::new(),
            look_overlay_scroll: 0,
            look_choice_request_index: 0,
            look_choice_option_index: 0,
            status_history: VecDeque::from([StatusEntry {
                code: None,
                message: "Ready. Press F1 for help.".to_string(),
            }]),
            event_history: VecDeque::new(),
            pending_choices: Vec::new(),
            pending_choices_state: ListState::default(),
            chat_history: VecDeque::new(),
            chat_history_scroll: 0,
            chat_history_viewport_height: 10,
            chat_history_total_lines: 0,
            pending_delivery_ids: HashSet::new(),
            terminal_delivery_message_ids: HashSet::new(),
            terminal_delivery_message_order: VecDeque::new(),
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
            other => self.push_status(None, other.to_string()),
        }
    }
}

fn map_relay_error(error: RelayError) -> RuntimeError {
    // Preserve the canonical terminal relay codes the tui-surface raww error
    // taxonomy enumerates as stable, machine-readable status codes: the
    // `validation_*` family, `relay_unavailable`, and `authorization_forbidden`.
    // Without the explicit `authorization_forbidden` arm a relay-enforced
    // permission denial collapses into a generic IO status with no code, even
    // though the spec requires it to surface terminally with its code intact.
    // Every other (internal) code stays a generic IO error so unexpected relay
    // internals do not leak a code the surface would treat as actionable.
    if error.code.starts_with("validation_")
        || error.code == "relay_unavailable"
        || error.code == "authorization_forbidden"
    {
        return RuntimeError::validation(error.code, error.message);
    }
    RuntimeError::io(
        error.message,
        io::Error::other("relay returned internal error"),
    )
}

fn map_relay_request_failure(socket_path: &std::path::Path, source: io::Error) -> RuntimeError {
    if is_relay_timeout_error(&source) {
        return RuntimeError::validation(
            "relay_timeout",
            format!(
                "relay timed out at {}; relay may be saturated or unresponsive",
                socket_path.display()
            ),
        );
    }
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

fn is_relay_timeout_error(source: &io::Error) -> bool {
    matches!(source.kind(), io::ErrorKind::TimedOut)
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
