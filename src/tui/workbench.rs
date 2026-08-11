//! Public workbench API for TUI event-driven integration.

use crossterm::event::Event;

use crate::relay::{RelayStreamEvent, SendResult};
use crate::runtime::error::RuntimeError;

use super::{
    input,
    state::{
        AppState, ChatHistoryDirection, ChatHistoryEntry, FocusField, PendingChoiceEntry,
        PickerColumn, Recipient, ScreenMode, TuiLaunchOptions,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkbenchField {
    To,
    Message,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkbenchMode {
    Communication,
    Interaction,
}

/// Which column of the unified picker currently holds focus. Mirrors the
/// internal `PickerColumn` for the public workbench/test surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkbenchPickerColumn {
    Bundles,
    Sessions,
}

/// Read-only projection of a pending choice row for the workbench/test surface,
/// mirroring the internal `PendingChoiceEntry` without exposing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkbenchPendingChoice {
    pub choice_request_id: String,
    pub message_id: Option<String>,
    pub target_session: Option<String>,
    pub requested_kind: Option<String>,
    pub option_ids: Vec<String>,
}

pub struct Workbench {
    state: AppState,
}

impl Workbench {
    pub fn new(options: TuiLaunchOptions) -> Self {
        Self {
            state: AppState::new(options),
        }
    }

    pub fn dispatch_event(&mut self, event: Event) -> Result<(), RuntimeError> {
        input::handle_event(&mut self.state, event)
    }

    pub fn set_focus(&mut self, field: WorkbenchField) {
        self.state.focus = match field {
            WorkbenchField::To => FocusField::To,
            WorkbenchField::Message => FocusField::Message,
        };
    }

    pub fn focus(&self) -> WorkbenchField {
        match self.state.focus {
            FocusField::To => WorkbenchField::To,
            FocusField::Message => WorkbenchField::Message,
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        self.state.insert_text(text);
    }

    pub fn to_field(&self) -> &str {
        self.state.to_field.as_str()
    }

    pub fn message_field(&self) -> &str {
        self.state.message_field.as_str()
    }

    pub fn to_cursor_column(&self) -> usize {
        self.state.to_cursor_column()
    }

    pub fn set_interaction_target(&mut self, target: &str) {
        self.state.set_interaction_target(target.to_string());
    }

    pub fn set_recipients(&mut self, sessions: &[&str]) {
        self.state.recipients = sessions
            .iter()
            .map(|session| Recipient {
                session_name: (*session).to_string(),
                display_name: None,
                ready: true,
            })
            .collect::<Vec<_>>();
        self.state.apply_recipient_list_update();
    }

    pub fn set_cross_bundle_candidates(&mut self, candidates: &[&str]) {
        self.state.cross_bundle_candidates = candidates
            .iter()
            .map(|candidate| (*candidate).to_string())
            .collect();
    }

    pub fn last_selected_recipient(&self) -> Option<&str> {
        self.state.last_selected_recipient.as_deref()
    }

    pub fn picker_selected_index(&self) -> Option<usize> {
        self.state.picker_session_state.selected()
    }

    pub fn message_cursor_line_and_column(&self) -> (usize, usize) {
        self.state.message_cursor_line_and_column()
    }

    pub fn inject_outgoing_history_entry(&mut self, body: &str) {
        self.state.chat_history.push_front(ChatHistoryEntry {
            direction: ChatHistoryDirection::Outgoing,
            peer_session: "relay".to_string(),
            body: body.to_string(),
        });
    }

    pub fn set_chat_history_viewport_height(&mut self, height: usize) {
        self.state.set_chat_history_viewport_height(height);
    }

    pub fn set_chat_history_total_lines(&mut self, total_lines: usize) {
        self.state.set_chat_history_total_lines(total_lines);
    }

    pub fn scroll_chat_history_page_up(&mut self) {
        self.state.scroll_chat_history_page_up();
    }

    pub fn chat_history_scroll(&self) -> usize {
        self.state.chat_history_scroll()
    }

    pub fn should_quit(&self) -> bool {
        self.state.should_quit
    }

    pub fn mode(&self) -> WorkbenchMode {
        match self.state.mode {
            ScreenMode::Communication => WorkbenchMode::Communication,
            ScreenMode::Interaction => WorkbenchMode::Interaction,
        }
    }

    pub fn interaction_target(&self) -> Option<&str> {
        self.state.look_target.as_deref()
    }

    pub fn raww_draft(&self) -> &str {
        self.state.raww_draft.as_str()
    }

    pub fn interaction_shows_raww(&self) -> bool {
        self.state.interaction_raww_region_visible()
    }

    pub fn picker_open(&self) -> bool {
        self.state.picker_open
    }

    pub fn picker_column(&self) -> WorkbenchPickerColumn {
        match self.state.picker_focus {
            PickerColumn::Bundles => WorkbenchPickerColumn::Bundles,
            PickerColumn::Sessions => WorkbenchPickerColumn::Sessions,
        }
    }

    pub fn picker_filter(&self) -> &str {
        self.state.picker_filter.as_str()
    }

    pub fn bundle_picker_selected_index(&self) -> Option<usize> {
        self.state.picker_bundle_state.selected()
    }

    pub fn namespace(&self) -> &str {
        self.state.namespace.as_str()
    }

    pub fn available_bundles(&self) -> Vec<&str> {
        self.state
            .available_bundles
            .iter()
            .map(String::as_str)
            .collect()
    }

    pub fn recipients(&self) -> Vec<&str> {
        self.state
            .recipients
            .iter()
            .map(|recipient| recipient.session_name.as_str())
            .collect()
    }

    pub fn inject_pending_choice(&mut self, target: &str) {
        self.state.pending_choices.push(PendingChoiceEntry {
            choice_request_id: format!("perm-{target}"),
            message_id: None,
            target_session: Some(target.to_string()),
            requested_kind: Some("approval".to_string()),
            requested_details: None,
            enqueued_at: None,
            options: Vec::new(),
        });
    }

    /// Feeds a `choices.requested` stream event through the real ingestion path
    /// (`record_stream_events` -> `upsert_pending_choice`) so the pending-choice
    /// ordering contract can be exercised end-to-end, including the
    /// `enqueued_at` sort that `inject_pending_choice` deliberately bypasses.
    pub fn inject_choice_request(
        &mut self,
        choice_request_id: &str,
        target_session: &str,
        enqueued_at: Option<&str>,
    ) {
        let mut payload = serde_json::json!({
            "choice_request_id": choice_request_id,
            "target_session": target_session,
            "requested_kind": "approval",
        });
        if let Some(enqueued_at) = enqueued_at {
            payload["enqueued_at"] = serde_json::Value::String(enqueued_at.to_string());
        }
        self.state.record_stream_events(&[RelayStreamEvent {
            event_type: "choices.requested".to_string(),
            target_session: target_session.to_string(),
            created_at: String::new(),
            payload,
        }]);
    }

    /// Ordered `choice_request_id`s in the pending-choices list, as the
    /// Interaction pane would present them.
    pub fn pending_choice_request_ids(&self) -> Vec<&str> {
        self.state
            .pending_choices
            .iter()
            .map(|entry| entry.choice_request_id.as_str())
            .collect()
    }

    pub fn scroll_chat_history_page_down(&mut self) {
        self.state.scroll_chat_history_page_down();
    }

    pub fn snap_chat_history_to_latest(&mut self) {
        self.state.snap_chat_history_to_latest();
    }

    /// Chat-history entry bodies in list order (front-most first).
    pub fn chat_history_bodies(&self) -> Vec<&str> {
        self.state
            .chat_history
            .iter()
            .map(|entry| entry.body.as_str())
            .collect()
    }

    /// Number of recorded activity-log events.
    pub fn event_history_len(&self) -> usize {
        self.state.event_history.len()
    }

    /// Recorded activity-log lines, newest first — the same order the events
    /// overlay renders them in.
    pub fn event_history_entries(&self) -> Vec<&str> {
        self.state
            .event_history
            .iter()
            .map(String::as_str)
            .collect()
    }

    /// Count of outstanding (not-yet-terminal) delivery acknowledgements.
    pub fn pending_deliveries_count(&self) -> usize {
        self.state.pending_deliveries_count()
    }

    /// Read-only projection of the pending-choices list for detailed inspection.
    pub fn pending_choices(&self) -> Vec<WorkbenchPendingChoice> {
        self.state
            .pending_choices
            .iter()
            .map(|entry| WorkbenchPendingChoice {
                choice_request_id: entry.choice_request_id.clone(),
                message_id: entry.message_id.clone(),
                target_session: entry.target_session.clone(),
                requested_kind: entry.requested_kind.clone(),
                option_ids: entry
                    .options
                    .iter()
                    .map(|option| option.option_id.clone())
                    .collect(),
            })
            .collect()
    }

    /// Pending-choice ids filtered to the active interaction target, as the
    /// Interaction pane's choice list presents them.
    pub fn look_pending_choice_request_ids(&self) -> Vec<&str> {
        self.state
            .look_pending_choices()
            .into_iter()
            .map(|entry| entry.choice_request_id.as_str())
            .collect()
    }

    /// Resolves the selected look choice as a selection, discarding the internal
    /// request payload so only the success/validation outcome is observable.
    pub fn resolve_selected_look_choice_selected(&mut self) -> Result<(), RuntimeError> {
        self.state.resolve_selected_look_choice_selected()
    }

    /// Resolves the selected look choice as a cancellation, discarding the
    /// internal request payload so only the success/validation outcome shows.
    pub fn resolve_selected_look_choice_cancelled(&mut self) -> Result<(), RuntimeError> {
        self.state.resolve_selected_look_choice_cancelled()
    }

    /// Feeds relay stream events through the real ingestion path
    /// (`record_stream_events`), the single seam TUI-integration tests use to
    /// drive `incoming_message`, `delivery_outcome`, and `choices.*` handling.
    /// Callers must build each `RelayStreamEvent` with the envelope/payload
    /// attribution that event type actually carries: `incoming_message` and
    /// `choices.*` stamp the receiving UI session in the envelope `target_session`
    /// and carry the canonical peer/target in the payload, whereas
    /// `delivery_outcome` names the delivery target in the envelope and carries
    /// no payload target.
    pub fn record_stream_events(&mut self, events: &[RelayStreamEvent]) {
        self.state.record_stream_events(events);
    }

    /// Records send results through the real chat-event path
    /// (`record_chat_events`), arming pending-delivery tracking keyed by
    /// `message_id` exactly as a live `Send` response would.
    pub fn record_chat_events(&mut self, results: &[SendResult]) {
        self.state.record_chat_events(results);
    }
}
