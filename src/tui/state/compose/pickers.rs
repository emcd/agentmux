use super::{AppState, FocusField, append_recipient_token};

impl AppState {
    pub fn move_picker_selection(&mut self, delta: isize) {
        if self.recipients.is_empty() {
            self.picker_state.select(None);
            return;
        }
        let current = self.picker_state.selected().unwrap_or(0);
        let next = super::text_util::wrap_index(current, delta, self.recipients.len());
        self.picker_state.select(Some(next));
    }

    pub fn open_picker(&mut self) {
        self.picker_open = true;
        self.bundle_picker_open = false;
        self.events_overlay_open = false;
        self.help_overlay_open = false;
        if self.recipients.is_empty() {
            self.picker_state.select(None);
            return;
        }
        let index = self.resolve_last_selected_recipient_index().unwrap_or(0);
        self.picker_state.select(Some(index));
    }

    pub fn close_picker(&mut self) {
        self.picker_open = false;
    }

    pub fn open_bundle_picker(&mut self) {
        self.bundle_picker_open = true;
        self.picker_open = false;
        self.events_overlay_open = false;
        self.help_overlay_open = false;
        if self.available_bundles.is_empty() {
            self.bundle_picker_state.select(None);
            return;
        }
        let active_index = self
            .available_bundles
            .iter()
            .position(|name| name == &self.bundle_name)
            .unwrap_or(0);
        self.bundle_picker_state.select(Some(active_index));
    }

    pub fn close_bundle_picker(&mut self) {
        self.bundle_picker_open = false;
    }

    pub fn move_bundle_picker_selection(&mut self, delta: isize) {
        if self.available_bundles.is_empty() {
            self.bundle_picker_state.select(None);
            return;
        }
        let current = self.bundle_picker_state.selected().unwrap_or(0);
        let next = super::text_util::wrap_index(current, delta, self.available_bundles.len());
        self.bundle_picker_state.select(Some(next));
    }

    pub fn toggle_events_overlay(&mut self) {
        self.events_overlay_open = !self.events_overlay_open;
        if self.events_overlay_open {
            self.picker_open = false;
            self.bundle_picker_open = false;
            self.help_overlay_open = false;
            self.ensure_pending_permission_selection();
        }
    }

    pub fn toggle_help_overlay(&mut self) {
        self.help_overlay_open = !self.help_overlay_open;
        if self.help_overlay_open {
            self.picker_open = false;
            self.bundle_picker_open = false;
            self.events_overlay_open = false;
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
        self.last_selected_recipient = Some(session_name.clone());
        self.picker_open = false;
        self.push_status(None, format!("Inserted recipient {session_name}."));
    }

    pub fn apply_recipient_list_update(&mut self) {
        if self.recipients.is_empty() {
            self.picker_state.select(None);
            return;
        }
        let index = self.resolve_last_selected_recipient_index().unwrap_or(0);
        self.picker_state.select(Some(index));
    }

    pub(super) fn selected_picker_recipient_id(&self) -> Option<String> {
        self.picker_state
            .selected()
            .and_then(|index| self.recipients.get(index))
            .map(|recipient| recipient.session_name.clone())
    }

    fn resolve_last_selected_recipient_index(&self) -> Option<usize> {
        let name = self.last_selected_recipient.as_deref()?;
        self.recipients
            .iter()
            .position(|recipient| recipient.session_name == name)
    }
}
