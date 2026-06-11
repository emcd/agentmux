use std::{
    collections::{HashSet, VecDeque},
    io,
    path::PathBuf,
};

use ratatui::widgets::ListState;

use crate::{
    acp::AcpSnapshotEntry,
    relay::{RelayError, RelayStreamSession},
    runtime::error::RuntimeError,
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
    AcpEntriesV1,
}

#[derive(Clone, Debug)]
pub struct TuiLaunchOptions {
    pub bundle_name: String,
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
pub(crate) struct PendingPermissionEntry {
    pub permission_request_id: String,
    pub message_id: Option<String>,
    pub target_session: Option<String>,
    pub requested_kind: Option<String>,
    pub requested_details: Option<serde_json::Value>,
    pub enqueued_at: Option<String>,
    pub options: Vec<PendingPermissionOption>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingPermissionOption {
    pub option_id: String,
    pub name: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug)]
pub(crate) struct AppState {
    pub bundle_name: String,
    pub sender_session: String,
    relay_socket: PathBuf,
    relay_stream: RelayStreamSession,
    look_lines: Option<u64>,
    pub recipients: Vec<Recipient>,
    pub bundle_status: Option<BundleStatusDisplay>,
    pub last_selected_recipient: Option<String>,
    pub available_bundles: Vec<String>,
    pub picker_open: bool,
    pub bundle_picker_open: bool,
    pub events_overlay_open: bool,
    pub help_overlay_open: bool,
    pub picker_state: ListState,
    pub bundle_picker_state: ListState,
    pub mode: ScreenMode,
    pub focus: FocusField,
    pub to_field: String,
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
    pub look_snapshot_entries: Vec<AcpSnapshotEntry>,
    pub look_overlay_scroll: usize,
    pub(crate) look_permission_request_index: usize,
    pub(crate) look_permission_option_index: usize,
    pub status_history: VecDeque<StatusEntry>,
    pub event_history: VecDeque<String>,
    pub pending_permissions: Vec<PendingPermissionEntry>,
    pub pending_permissions_state: ListState,
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
            bundle_name,
            sender_session,
            relay_socket,
            look_lines,
            available_bundles,
        } = options;
        let relay_stream = RelayStreamSession::new(
            relay_socket.clone(),
            bundle_name.clone(),
            sender_session.clone(),
        );
        Self {
            bundle_name,
            sender_session,
            relay_socket,
            relay_stream,
            look_lines,
            recipients: Vec::new(),
            bundle_status: None,
            last_selected_recipient: None,
            available_bundles,
            picker_open: false,
            bundle_picker_open: false,
            events_overlay_open: false,
            help_overlay_open: false,
            picker_state: ListState::default(),
            bundle_picker_state: ListState::default(),
            mode: ScreenMode::Communication,
            focus: FocusField::To,
            to_field: String::new(),
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
            look_permission_request_index: 0,
            look_permission_option_index: 0,
            status_history: VecDeque::from([StatusEntry {
                code: None,
                message: "Ready. Press F1 for help.".to_string(),
            }]),
            event_history: VecDeque::new(),
            pending_permissions: Vec::new(),
            pending_permissions_state: ListState::default(),
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
    if error.code.starts_with("validation_") || error.code == "relay_unavailable" {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use crate::{
        relay::{RelayStreamEvent, SendOutcome, SendResult},
        runtime::error::RuntimeError,
    };

    use super::{AppState, PendingPermissionEntry, TuiLaunchOptions};

    fn make_state() -> AppState {
        AppState::new(TuiLaunchOptions {
            bundle_name: "agentmux".to_string(),
            sender_session: "tui".to_string(),
            relay_socket: PathBuf::from("/tmp/agentmux-test-relay.sock"),
            look_lines: None,
            available_bundles: vec!["agentmux".to_string()],
        })
    }

    #[test]
    fn chat_history_scroll_paging_uses_line_units() {
        let mut state = make_state();
        state.set_chat_history_viewport_height(4);
        state.set_chat_history_total_lines(10);

        state.scroll_chat_history_page_up();
        assert_eq!(state.chat_history_scroll(), 4);

        state.scroll_chat_history_page_up();
        assert_eq!(state.chat_history_scroll(), 6);

        state.scroll_chat_history_page_down();
        assert_eq!(state.chat_history_scroll(), 2);

        state.snap_chat_history_to_latest();
        assert_eq!(state.chat_history_scroll(), 0);
    }

    #[test]
    fn record_stream_events_deduplicates_incoming_message_ids() {
        let mut state = make_state();
        let duplicated = RelayStreamEvent {
            event_type: "incoming_message".to_string(),
            target_session: "tui@agentmux".to_string(),
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
    fn tui_starts_in_communication_mode() {
        let state = make_state();
        assert_eq!(state.mode, super::ScreenMode::Communication);
    }

    #[test]
    fn toggle_mode_switches_between_communication_and_interaction() {
        let mut state = make_state();
        assert_eq!(state.mode, super::ScreenMode::Communication);
        state.toggle_mode();
        assert_eq!(state.mode, super::ScreenMode::Interaction);
        state.toggle_mode();
        assert_eq!(state.mode, super::ScreenMode::Communication);
    }

    #[test]
    fn mode_switch_preserves_compose_and_raww_drafts() {
        let mut state = make_state();
        state.to_field = "user".to_string();
        state.message_field = "hello".to_string();
        state.raww_draft = "echo".to_string();
        state.raww_cursor_index = state.raww_draft.len();
        state.toggle_mode();
        state.toggle_mode();
        assert_eq!(state.to_field, "user");
        assert_eq!(state.message_field, "hello");
        assert_eq!(state.raww_draft, "echo");
        assert_eq!(state.raww_cursor_index, 4);
    }

    #[test]
    fn terminal_delivery_outcome_removes_pending_message() {
        let mut state = make_state();
        state.record_chat_events(&[SendResult {
            target_session: "user".to_string(),
            message_id: "msg-1".to_string(),
            outcome: SendOutcome::Queued,
            reason_code: None,
            reason: None,
            details: None,
        }]);
        assert_eq!(state.pending_deliveries_count(), 1);

        state.record_stream_events(&[RelayStreamEvent {
            event_type: "delivery_outcome".to_string(),
            target_session: "user@agentmux".to_string(),
            created_at: "2026-03-29T00:00:00Z".to_string(),
            payload: json!({
                "message_id": "msg-1",
                "phase": "delivered",
                "outcome": "success",
            }),
        }]);
        assert_eq!(state.pending_deliveries_count(), 0);
    }

    #[test]
    fn queued_result_does_not_readd_pending_after_terminal_outcome_arrives_first() {
        let mut state = make_state();
        state.record_stream_events(&[RelayStreamEvent {
            event_type: "delivery_outcome".to_string(),
            target_session: "user@agentmux".to_string(),
            created_at: "2026-03-29T00:00:00Z".to_string(),
            payload: json!({
                "message_id": "msg-1",
                "phase": "delivered",
                "outcome": "success",
            }),
        }]);
        assert_eq!(state.pending_deliveries_count(), 0);

        state.record_chat_events(&[SendResult {
            target_session: "user".to_string(),
            message_id: "msg-1".to_string(),
            outcome: SendOutcome::Queued,
            reason_code: None,
            reason: None,
            details: None,
        }]);
        assert_eq!(state.pending_deliveries_count(), 0);
    }

    #[test]
    fn permission_snapshot_and_replay_keep_single_pending_row_per_id() {
        let mut state = make_state();
        state.record_stream_events(&[RelayStreamEvent {
            event_type: "permission.snapshot".to_string(),
            target_session: "tui@agentmux".to_string(),
            created_at: "2026-04-29T00:00:00Z".to_string(),
            payload: json!({
                "pending_count": 1,
                "permission_request_ids": ["perm-1"],
            }),
        }]);
        assert_eq!(state.pending_permissions.len(), 1);
        assert_eq!(state.pending_permissions[0].permission_request_id, "perm-1");

        let requested = RelayStreamEvent {
            event_type: "permission.requested".to_string(),
            target_session: "tui@agentmux".to_string(),
            created_at: "2026-04-29T00:00:01Z".to_string(),
            payload: json!({
                "message_id": "msg-1",
                "permission_request_id": "perm-1",
                "target_session": "acp",
                "requested_kind": "approval",
                "requested_details": {
                    "prompt": "run command",
                    "options": [
                        {"option_id": "allow-once", "name": "Allow once", "kind": "allow_once"},
                        {"option_id": "reject-once", "name": "Reject", "kind": "reject_once"}
                    ]
                },
                "enqueued_at": "2026-04-29T00:00:01Z",
            }),
        };
        state.record_stream_events(&[requested.clone(), requested]);
        assert_eq!(state.pending_permissions.len(), 1);
        let entry = &state.pending_permissions[0];
        assert_eq!(entry.permission_request_id, "perm-1");
        assert_eq!(entry.message_id.as_deref(), Some("msg-1"));
        assert_eq!(entry.target_session.as_deref(), Some("acp"));
        assert_eq!(entry.requested_kind.as_deref(), Some("approval"));
        assert_eq!(entry.options.len(), 2);
        assert_eq!(entry.options[0].option_id, "allow-once");
        assert_eq!(entry.options[1].option_id, "reject-once");
    }

    #[test]
    fn permission_resolved_removes_pending_request() {
        let mut state = make_state();
        state.record_stream_events(&[RelayStreamEvent {
            event_type: "permission.requested".to_string(),
            target_session: "tui@agentmux".to_string(),
            created_at: "2026-04-29T00:00:01Z".to_string(),
            payload: json!({
                "message_id": "msg-1",
                "permission_request_id": "perm-1",
                "target_session": "acp",
                "requested_kind": "approval",
                "requested_details": {
                    "prompt": "run command",
                    "options": [{"option_id": "allow-once", "name": "Allow once", "kind": "allow_once"}]
                },
                "enqueued_at": "2026-04-29T00:00:01Z",
            }),
        }]);
        assert_eq!(state.pending_permissions.len(), 1);

        state.record_stream_events(&[RelayStreamEvent {
            event_type: "permission.resolved".to_string(),
            target_session: "tui@agentmux".to_string(),
            created_at: "2026-04-29T00:00:02Z".to_string(),
            payload: json!({
                "message_id": "msg-1",
                "permission_request_id": "perm-1",
                "outcome": "selected",
                "reason_code": null,
                "decided_by": "user",
                "reason": null,
                "resolved_at": "2026-04-29T00:00:02Z",
            }),
        }]);
        assert!(state.pending_permissions.is_empty());
    }

    #[test]
    fn look_permission_resolve_without_pending_request_is_validation_error() {
        let mut state = make_state();
        state.look_target = Some("acp".to_string());
        let selected = state.resolve_selected_look_permission_selected();
        match selected {
            Err(RuntimeError::Validation { code, .. }) => {
                assert_eq!(code, "validation_unknown_permission_request");
            }
            other => panic!("unexpected result: {other:?}"),
        }

        let cancelled = state.resolve_selected_look_permission_cancelled();
        match cancelled {
            Err(RuntimeError::Validation { code, .. }) => {
                assert_eq!(code, "validation_unknown_permission_request");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn look_pending_permissions_filter_by_active_target_session() {
        let mut state = make_state();
        state.pending_permissions = vec![
            PendingPermissionEntry {
                permission_request_id: "perm-1".to_string(),
                message_id: Some("msg-1".to_string()),
                target_session: Some("acp".to_string()),
                requested_kind: Some("approval".to_string()),
                requested_details: None,
                options: vec![],
                enqueued_at: Some("2026-04-29T00:00:01Z".to_string()),
            },
            PendingPermissionEntry {
                permission_request_id: "perm-2".to_string(),
                message_id: Some("msg-2".to_string()),
                target_session: Some("relay".to_string()),
                requested_kind: Some("approval".to_string()),
                requested_details: None,
                options: vec![],
                enqueued_at: Some("2026-04-29T00:00:02Z".to_string()),
            },
        ];
        state.look_target = Some("acp".to_string());
        let filtered = state.look_pending_permissions();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].permission_request_id, "perm-1");
    }
}
