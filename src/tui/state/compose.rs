use crate::{
    relay::{ListedSessionTransport, LookSnapshotPayload, RelayRequest, RelayResponse},
    runtime::error::RuntimeError,
};

use super::{
    AppState, FocusField, LookSnapshotFormat, PendingPermissionOption, ScreenMode,
    ToCompletionState, append_recipient_token, current_recipient_token_context, map_relay_error,
    matching_recipient_candidates,
};

impl AppState {
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
        let next = wrap_index(current, delta, self.available_bundles.len());
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

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            ScreenMode::Communication => ScreenMode::Interaction,
            ScreenMode::Interaction => ScreenMode::Communication,
        };
    }

    pub fn enter_interaction_mode(&mut self) {
        self.mode = ScreenMode::Interaction;
    }

    pub fn scroll_interaction_snapshot_up(&mut self) {
        self.look_overlay_scroll = self.look_overlay_scroll.saturating_add(1);
    }

    pub fn scroll_interaction_snapshot_down(&mut self) {
        self.look_overlay_scroll = self.look_overlay_scroll.saturating_sub(1);
    }

    pub fn scroll_interaction_snapshot_page_up(&mut self) {
        self.look_overlay_scroll = self.look_overlay_scroll.saturating_add(10);
    }

    pub fn scroll_interaction_snapshot_page_down(&mut self) {
        self.look_overlay_scroll = self.look_overlay_scroll.saturating_sub(10);
    }

    pub fn toggle_help_overlay(&mut self) {
        self.help_overlay_open = !self.help_overlay_open;
        if self.help_overlay_open {
            self.picker_open = false;
            self.bundle_picker_open = false;
            self.events_overlay_open = false;
        }
    }

    pub fn move_look_permission_request_selection(&mut self, delta: isize) {
        let entries = self.look_pending_permissions();
        if entries.is_empty() {
            self.look_permission_request_index = 0;
            self.look_permission_option_index = 0;
            return;
        }
        self.look_permission_request_index =
            wrap_index(self.look_permission_request_index, delta, entries.len());
        self.look_permission_option_index = 0;
    }

    pub fn move_look_permission_option_selection(&mut self, delta: isize) {
        let Some(entry) = self.selected_look_permission() else {
            self.look_permission_option_index = 0;
            return;
        };
        if entry.options.is_empty() {
            self.look_permission_option_index = 0;
            return;
        }
        self.look_permission_option_index = wrap_index(
            self.look_permission_option_index,
            delta,
            entry.options.len(),
        );
    }

    pub fn resolve_selected_look_permission_selected(&mut self) -> Result<(), RuntimeError> {
        let Some(entry) = self.selected_look_permission() else {
            return Err(RuntimeError::validation(
                "validation_unknown_permission_request",
                "selected outcome requires a pending permission request for the current look target",
            ));
        };
        let Some(option) = self.selected_look_permission_option() else {
            return Err(RuntimeError::validation(
                "validation_invalid_params",
                "selected outcome requires explicit option selection",
            ));
        };
        self.submit_permission_decision(
            entry.permission_request_id.clone(),
            "selected",
            Some(option.option_id.clone()),
        )
    }

    pub fn resolve_selected_look_permission_cancelled(&mut self) -> Result<(), RuntimeError> {
        let permission_request_id = self
            .selected_look_permission()
            .map(|entry| entry.permission_request_id.clone())
            .ok_or_else(|| {
            RuntimeError::validation(
                "validation_unknown_permission_request",
                "cancelled outcome requires a pending permission request for the current look target",
            )
        })?;
        self.submit_permission_decision(permission_request_id, "cancelled", None)
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
        let Some(completion_state) = self.to_completion.as_ref() else {
            return false;
        };
        self.commit_completed_to_token(completion_state.token_start);
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

    pub fn move_message_cursor_left(&mut self) {
        if self.focus != FocusField::Message || self.message_cursor_index == 0 {
            return;
        }
        self.message_cursor_index =
            previous_char_boundary(self.message_field.as_str(), self.message_cursor_index);
        self.message_cursor_preferred_column = None;
    }

    pub fn move_message_cursor_right(&mut self) {
        if self.focus != FocusField::Message
            || self.message_cursor_index >= self.message_field.len()
        {
            return;
        }
        self.message_cursor_index =
            next_char_boundary(self.message_field.as_str(), self.message_cursor_index);
        self.message_cursor_preferred_column = None;
    }

    pub fn move_message_cursor_home(&mut self) {
        if self.focus != FocusField::Message {
            return;
        }
        let (line_start, _) =
            line_range_for_cursor(self.message_field.as_str(), self.message_cursor_index);
        self.message_cursor_index = line_start;
        self.message_cursor_preferred_column = None;
    }

    pub fn move_message_cursor_end(&mut self) {
        if self.focus != FocusField::Message {
            return;
        }
        let (_, line_end) =
            line_range_for_cursor(self.message_field.as_str(), self.message_cursor_index);
        self.message_cursor_index = line_end;
        self.message_cursor_preferred_column = None;
    }

    pub fn message_cursor_index(&self) -> usize {
        self.message_cursor_index
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

    fn commit_completed_to_token(&mut self, token_start: usize) {
        let Some(token_slice) = self.to_field.get(token_start..) else {
            return;
        };
        let raw_token = token_slice
            .split(',')
            .next()
            .map(str::trim_end)
            .unwrap_or(token_slice);
        let token_end = token_start + raw_token.len();
        let Some(trailing) = self.to_field.get(token_end..) else {
            return;
        };

        if trailing.is_empty() {
            self.to_field.push_str(", ");
            return;
        }

        if trailing.starts_with(',') {
            return;
        }

        self.to_field.insert(token_end, ',');
        self.to_field.insert(token_end + 1, ' ');
    }

    fn clear_to_completion(&mut self) {
        self.to_completion = None;
    }

    pub(super) fn clear_compose_fields(&mut self) {
        self.to_field.clear();
        self.message_field.clear();
        self.message_cursor_index = 0;
        self.message_cursor_preferred_column = None;
        self.focus = FocusField::To;
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

    pub fn enter_interaction_from_picker(&mut self) -> Result<(), RuntimeError> {
        let target = self.selected_picker_recipient_id().ok_or_else(|| {
            RuntimeError::validation(
                "validation_unknown_target",
                "interaction requires a selected recipient in picker",
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
                snapshot,
                ..
            } => {
                let (look_snapshot_format, look_snapshot_lines, look_snapshot_entries) =
                    overlay_snapshot_from_payload(snapshot);
                self.last_selected_recipient = Some(target_session.clone());
                self.set_interaction_target(target_session.clone());
                self.look_captured_at = Some(captured_at);
                self.look_snapshot_format = Some(look_snapshot_format);
                self.look_snapshot_lines = look_snapshot_lines;
                self.look_snapshot_entries = look_snapshot_entries;
                self.picker_open = false;
                self.enter_interaction_mode();
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

    pub fn dispatch_raww_from_interaction(&mut self) -> Result<(), RuntimeError> {
        let Some(target) = self.look_target.clone() else {
            return Err(RuntimeError::validation(
                "validation_unknown_target",
                "write requires an active interaction target",
            ));
        };
        if self.raww_draft.trim().is_empty() {
            return Err(RuntimeError::validation(
                "validation_missing_message_input",
                "write text is required from Write input pane",
            ));
        }

        let text = self.raww_draft.clone();
        let response = self.request_relay(&RelayRequest::Raww {
            request_id: None,
            sender_session: self.sender_session.clone(),
            target_session: target.clone(),
            text,
            no_enter: false,
            bundle_name: None,
        })?;

        match response {
            RelayResponse::Raww {
                status,
                target_session,
                transport,
                request_id: _,
                message_id,
                details,
                ..
            } => {
                let transport_label = render_transport_label(transport);
                let phase = details
                    .as_ref()
                    .and_then(|value| value.get("delivery_phase"))
                    .and_then(serde_json::Value::as_str);
                let message_id_label = message_id.as_deref().unwrap_or("-");

                if let Some(phase) = phase {
                    self.push_status(
                        None,
                        format!(
                            "write accepted status={status} target={target_session} transport={transport_label} phase={phase}"
                        ),
                    );
                    self.push_event(format!(
                        "write target={target_session} status={status} transport={transport_label} phase={phase} message_id={message_id_label}"
                    ));
                } else {
                    self.push_status(
                        None,
                        format!(
                            "write accepted status={status} target={target_session} transport={transport_label}"
                        ),
                    );
                    self.push_event(format!(
                        "write target={target_session} status={status} transport={transport_label} message_id={message_id_label}"
                    ));
                }
                self.clear_raww_draft();
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

    pub fn set_interaction_target(&mut self, target: String) {
        let target_changed = self.look_target.as_deref() != Some(target.as_str());
        self.look_target = Some(target);
        if target_changed {
            self.look_overlay_scroll = 0;
            self.look_permission_request_index = 0;
            self.look_permission_option_index = 0;
        }
    }

    pub fn insert_character_in_raww(&mut self, character: char) {
        self.raww_draft.insert(self.raww_cursor_index, character);
        self.raww_cursor_index += character.len_utf8();
        self.raww_cursor_preferred_column = None;
    }

    pub fn insert_newline_in_raww(&mut self) {
        self.insert_character_in_raww('\n');
    }

    pub fn backspace_raww(&mut self) {
        if self.raww_cursor_index == 0 {
            return;
        }
        let next_cursor = previous_char_boundary(self.raww_draft.as_str(), self.raww_cursor_index);
        self.raww_draft
            .replace_range(next_cursor..self.raww_cursor_index, "");
        self.raww_cursor_index = next_cursor;
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_left(&mut self) {
        if self.raww_cursor_index == 0 {
            return;
        }
        self.raww_cursor_index =
            previous_char_boundary(self.raww_draft.as_str(), self.raww_cursor_index);
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_right(&mut self) {
        if self.raww_cursor_index >= self.raww_draft.len() {
            return;
        }
        self.raww_cursor_index =
            next_char_boundary(self.raww_draft.as_str(), self.raww_cursor_index);
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_home(&mut self) {
        let (line_start, _) =
            line_range_for_cursor(self.raww_draft.as_str(), self.raww_cursor_index);
        self.raww_cursor_index = line_start;
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_end(&mut self) {
        let (_, line_end) = line_range_for_cursor(self.raww_draft.as_str(), self.raww_cursor_index);
        self.raww_cursor_index = line_end;
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_up(&mut self) {
        self.move_raww_cursor_vertical(-1);
    }

    pub fn move_raww_cursor_down(&mut self) {
        self.move_raww_cursor_vertical(1);
    }

    fn move_raww_cursor_vertical(&mut self, delta: isize) {
        let line_ranges = line_ranges(self.raww_draft.as_str());
        if line_ranges.is_empty() {
            return;
        }
        let (current_line, current_column) =
            line_and_column_for_index(self.raww_draft.as_str(), self.raww_cursor_index);
        let target_line = if delta.is_negative() {
            current_line.saturating_sub(delta.unsigned_abs())
        } else {
            (current_line + delta as usize).min(line_ranges.len().saturating_sub(1))
        };
        if target_line == current_line {
            return;
        }
        let preferred_column = self.raww_cursor_preferred_column.unwrap_or(current_column);
        self.raww_cursor_index = cursor_index_for_line_column(
            self.raww_draft.as_str(),
            line_ranges[target_line],
            preferred_column,
        );
        self.raww_cursor_preferred_column = Some(preferred_column);
    }

    pub fn raww_cursor_line_and_column(&self) -> (usize, usize) {
        line_and_column_for_index(self.raww_draft.as_str(), self.raww_cursor_index)
    }

    pub fn clear_raww_draft(&mut self) {
        self.raww_draft.clear();
        self.raww_cursor_index = 0;
        self.raww_cursor_preferred_column = None;
    }

    fn selected_picker_recipient_id(&self) -> Option<String> {
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

    pub fn apply_recipient_list_update(&mut self) {
        if self.recipients.is_empty() {
            self.picker_state.select(None);
            return;
        }
        let index = self.resolve_last_selected_recipient_index().unwrap_or(0);
        self.picker_state.select(Some(index));
    }

    pub(super) fn ensure_pending_permission_selection(&mut self) {
        if self.pending_permissions.is_empty() {
            self.pending_permissions_state.select(None);
            return;
        }
        let selected = self
            .pending_permissions_state
            .selected()
            .filter(|index| *index < self.pending_permissions.len())
            .unwrap_or(0);
        self.pending_permissions_state.select(Some(selected));
    }

    fn submit_permission_decision(
        &mut self,
        permission_request_id: String,
        outcome: &str,
        option_id: Option<String>,
    ) -> Result<(), RuntimeError> {
        if outcome == "selected" && option_id.is_none() {
            return Err(RuntimeError::validation(
                "validation_invalid_params",
                "selected outcome requires explicit option_id",
            ));
        }
        if outcome == "cancelled" && option_id.is_some() {
            return Err(RuntimeError::validation(
                "validation_invalid_params",
                "cancelled outcome must omit option_id",
            ));
        }
        let request = RelayRequest::PermissionResolve {
            permission_request_id: permission_request_id.clone(),
            outcome: outcome.to_string(),
            option_id,
            bundle_name: None,
            ui_session_id: None,
        };
        match self.request_relay(&request)? {
            RelayResponse::PermissionDecision {
                status,
                permission_request_id,
                outcome,
                reason_code,
                reason,
                ..
            } => {
                let reason_code_label = reason_code.as_deref().unwrap_or("-");
                if let Some(reason) = reason.as_deref() {
                    self.push_status(
                        None,
                        format!(
                            "permission decision status={status} id={permission_request_id} outcome={outcome} reason_code={reason_code_label} reason={reason}"
                        ),
                    );
                } else {
                    self.push_status(
                        None,
                        format!(
                            "permission decision status={status} id={permission_request_id} outcome={outcome} reason_code={reason_code_label}"
                        ),
                    );
                }
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

    pub(crate) fn interaction_raww_region_visible(&self) -> bool {
        !self.raww_draft.is_empty() || self.look_pending_permissions().is_empty()
    }

    pub(crate) fn look_pending_permissions(&self) -> Vec<&super::PendingPermissionEntry> {
        let Some(look_target) = self.look_target.as_deref() else {
            return Vec::new();
        };
        self.pending_permissions
            .iter()
            .filter(|entry| entry.target_session.as_deref() == Some(look_target))
            .collect::<Vec<_>>()
    }

    pub(super) fn selected_look_permission(&self) -> Option<&super::PendingPermissionEntry> {
        let entries = self.look_pending_permissions();
        if entries.is_empty() {
            return None;
        }
        entries
            .get(
                self.look_permission_request_index
                    .min(entries.len().saturating_sub(1)),
            )
            .copied()
    }

    pub(super) fn selected_look_permission_option(&self) -> Option<&PendingPermissionOption> {
        let entry = self.selected_look_permission()?;
        if entry.options.is_empty() {
            return None;
        }
        entry.options.get(
            self.look_permission_option_index
                .min(entry.options.len().saturating_sub(1)),
        )
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

fn next_char_boundary(value: &str, cursor_index: usize) -> usize {
    value
        .char_indices()
        .find_map(|(index, _)| (index > cursor_index).then_some(index))
        .unwrap_or(value.len())
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

fn line_range_for_cursor(value: &str, cursor_index: usize) -> (usize, usize) {
    let cursor_index = cursor_index.min(value.len());
    let ranges = line_ranges(value);
    for (line_index, (line_start, line_end)) in ranges.iter().enumerate() {
        if cursor_index <= *line_end || line_index + 1 == ranges.len() {
            return (*line_start, *line_end);
        }
    }
    (0, value.len())
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

fn overlay_snapshot_from_payload(
    snapshot: LookSnapshotPayload,
) -> (
    LookSnapshotFormat,
    Vec<String>,
    Vec<crate::acp::AcpSnapshotEntry>,
) {
    match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => {
            (LookSnapshotFormat::Lines, snapshot_lines, Vec::new())
        }
        LookSnapshotPayload::AcpEntriesV1 {
            snapshot_entries, ..
        } => (
            LookSnapshotFormat::AcpEntriesV1,
            Vec::new(),
            snapshot_entries,
        ),
    }
}

fn render_transport_label(transport: ListedSessionTransport) -> &'static str {
    match transport {
        ListedSessionTransport::Tmux => "tmux",
        ListedSessionTransport::Acp => "acp",
        ListedSessionTransport::Ui => "ui",
        ListedSessionTransport::Pubsub => "pubsub",
    }
}
