//! Public workbench API for TUI event-driven integration.

use crossterm::event::Event;

use crate::runtime::error::RuntimeError;

use super::{
    input,
    state::{
        AppState, ChatHistoryDirection, ChatHistoryEntry, FocusField, PendingChoiceEntry,
        Recipient, ScreenMode, TuiLaunchOptions,
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

    pub fn last_selected_recipient(&self) -> Option<&str> {
        self.state.last_selected_recipient.as_deref()
    }

    pub fn picker_selected_index(&self) -> Option<usize> {
        self.state.picker_state.selected()
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

    pub fn bundle_picker_open(&self) -> bool {
        self.state.bundle_picker_open
    }

    pub fn bundle_picker_selected_index(&self) -> Option<usize> {
        self.state.bundle_picker_state.selected()
    }

    pub fn bundle_name(&self) -> &str {
        self.state.bundle_name.as_str()
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
}
