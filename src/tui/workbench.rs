//! Public workbench API for TUI event-driven integration.

use crossterm::event::Event;

use crate::runtime::error::RuntimeError;

use super::{
    input,
    state::{
        AppState, ChatHistoryDirection, ChatHistoryEntry, FocusField, PendingPermissionEntry,
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

    pub fn set_recipients(&mut self, sessions: &[&str]) {
        self.state.recipients = sessions
            .iter()
            .map(|session| Recipient {
                session_name: (*session).to_string(),
                display_name: None,
            })
            .collect::<Vec<_>>();
    }

    pub fn message_cursor_line_and_column(&self) -> (usize, usize) {
        self.state.message_cursor_line_and_column()
    }

    pub fn inject_outgoing_history_entry(&mut self, body: &str) {
        self.state.chat_history.push_front(ChatHistoryEntry {
            direction: ChatHistoryDirection::Outgoing,
            peer_session: "relay".to_string(),
            body: body.to_string(),
            message_id: None,
        });
    }

    pub fn set_chat_history_viewport_height(&mut self, height: usize) {
        self.state.set_chat_history_viewport_height(height);
    }

    pub fn scroll_chat_history_page_up(&mut self) {
        self.state.scroll_chat_history_page_up();
    }

    pub fn visible_chat_history_bodies(&self) -> Vec<String> {
        self.state
            .visible_chat_history_entries()
            .into_iter()
            .map(|entry| entry.body)
            .collect::<Vec<_>>()
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

    pub fn inject_pending_permission(&mut self, target: &str) {
        self.state.pending_permissions.push(PendingPermissionEntry {
            permission_request_id: format!("perm-{target}"),
            message_id: None,
            target_session: Some(target.to_string()),
            requested_kind: Some("approval".to_string()),
            requested_details: None,
            enqueued_at: None,
            options: Vec::new(),
        });
    }
}
