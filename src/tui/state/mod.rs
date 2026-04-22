use std::{
    collections::{HashSet, VecDeque},
    io,
    path::PathBuf,
};

use ratatui::widgets::ListState;

use crate::{
    acp::AcpSnapshotEntry,
    relay::{RelayError, RelayStreamClientClass, RelayStreamSession},
    runtime::error::RuntimeError,
};

use super::target::{
    ToCompletionState, append_recipient_token, current_recipient_token_context,
    matching_recipient_candidates, merge_tui_targets,
};

mod compose;
mod history;
mod relay;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Recipient {
    pub session_name: String,
    pub display_name: Option<String>,
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
    pub look_snapshot_format: Option<LookSnapshotFormat>,
    pub look_snapshot_lines: Vec<String>,
    pub look_snapshot_entries: Vec<AcpSnapshotEntry>,
    pub look_overlay_scroll: usize,
    pub status_history: VecDeque<StatusEntry>,
    pub event_history: VecDeque<String>,
    pub chat_history: VecDeque<ChatHistoryEntry>,
    chat_history_scroll: usize,
    chat_history_viewport_height: usize,
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
            look_snapshot_format: None,
            look_snapshot_lines: Vec::new(),
            look_snapshot_entries: Vec::new(),
            look_overlay_scroll: 0,
            status_history: VecDeque::from([StatusEntry {
                code: None,
                message: "Ready. Press F1 for help.".to_string(),
            }]),
            event_history: VecDeque::new(),
            chat_history: VecDeque::new(),
            chat_history_scroll: 0,
            chat_history_viewport_height: 10,
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

    use crate::relay::{ChatOutcome, ChatResult, ChatStatus, RelayStreamEvent};

    use super::{AppState, ChatHistoryDirection, ChatHistoryEntry, Recipient, TuiLaunchOptions};

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
        state.recipients = vec![Recipient {
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

    #[test]
    fn terminal_delivery_outcome_removes_pending_message() {
        let mut state = make_state();
        state.record_chat_events(
            &ChatStatus::Accepted,
            &[ChatResult {
                target_session: "user".to_string(),
                message_id: "msg-1".to_string(),
                outcome: ChatOutcome::Queued,
                reason_code: None,
                reason: None,
                details: None,
            }],
        );
        assert_eq!(state.pending_deliveries_count(), 1);

        state.record_stream_events(&[RelayStreamEvent {
            event_type: "delivery_outcome".to_string(),
            bundle_name: "agentmux".to_string(),
            target_session: "user".to_string(),
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
            bundle_name: "agentmux".to_string(),
            target_session: "user".to_string(),
            created_at: "2026-03-29T00:00:00Z".to_string(),
            payload: json!({
                "message_id": "msg-1",
                "phase": "delivered",
                "outcome": "success",
            }),
        }]);
        assert_eq!(state.pending_deliveries_count(), 0);

        state.record_chat_events(
            &ChatStatus::Accepted,
            &[ChatResult {
                target_session: "user".to_string(),
                message_id: "msg-1".to_string(),
                outcome: ChatOutcome::Queued,
                reason_code: None,
                reason: None,
                details: None,
            }],
        );
        assert_eq!(state.pending_deliveries_count(), 0);
    }
}
