use crate::{
    relay::{RelayRequest, RelayResponse},
    runtime::error::RuntimeError,
};

use super::{
    AppState, FocusField, ToCompletionState, append_recipient_token,
    current_recipient_token_context, map_relay_error, matching_recipient_candidates,
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
        self.events_overlay_open = false;
        self.look_overlay_open = false;
        self.look_overlay_restore_picker_on_close = false;
        self.help_overlay_open = false;
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
            self.look_overlay_open = false;
            self.look_overlay_restore_picker_on_close = false;
            self.help_overlay_open = false;
        }
    }

    pub fn open_look_overlay(&mut self) {
        self.look_overlay_restore_picker_on_close = self.picker_open;
        self.look_overlay_open = true;
        self.picker_open = false;
        self.events_overlay_open = false;
        self.help_overlay_open = false;
    }

    pub fn close_look_overlay(&mut self) {
        self.look_overlay_open = false;
        let restore_picker = self.look_overlay_restore_picker_on_close;
        self.look_overlay_restore_picker_on_close = false;
        if restore_picker {
            self.open_picker();
        }
    }

    pub fn toggle_help_overlay(&mut self) {
        self.help_overlay_open = !self.help_overlay_open;
        if self.help_overlay_open {
            self.picker_open = false;
            self.events_overlay_open = false;
            self.look_overlay_open = false;
            self.look_overlay_restore_picker_on_close = false;
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

    pub fn look_picker_target(&mut self) -> Result<(), RuntimeError> {
        let target = self.selected_picker_recipient_id().ok_or_else(|| {
            RuntimeError::validation(
                "validation_unknown_target",
                "look requires a selected recipient in picker",
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
                self.open_look_overlay();
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

    fn selected_picker_recipient_id(&self) -> Option<String> {
        self.picker_state
            .selected()
            .and_then(|index| self.recipients.get(index))
            .map(|recipient| recipient.session_name.clone())
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
