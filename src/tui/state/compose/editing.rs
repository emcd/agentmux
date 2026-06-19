use super::{
    AppState, FocusField, ScreenMode, ToCompletionState, current_recipient_token_context,
    matching_recipient_candidates,
};

impl AppState {
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

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            ScreenMode::Communication => ScreenMode::Interaction,
            ScreenMode::Interaction => ScreenMode::Communication,
        };
        // Entering Interaction without an active session leaves the operator on
        // an empty target header with nothing to write to. Auto-open the unified
        // picker (session column) so a target can be chosen immediately.
        if self.mode == ScreenMode::Interaction && self.look_target.is_none() {
            self.open_picker();
        }
    }

    pub fn insert_character(&mut self, character: char) {
        match self.focus {
            FocusField::To => {
                self.to_field.insert(self.to_cursor_index, character);
                self.to_cursor_index += character.len_utf8();
                self.on_to_field_edited();
                // Token completion operates on the trailing recipient token, so
                // it only applies when editing at the end of the field.
                if self.to_cursor_index == self.to_field.len() {
                    self.maybe_autocomplete_at_prefixed_token();
                }
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
                if self.to_cursor_index == 0 {
                    return;
                }
                let next_cursor = super::text_util::previous_char_boundary(
                    self.to_field.as_str(),
                    self.to_cursor_index,
                );
                self.to_field
                    .replace_range(next_cursor..self.to_cursor_index, "");
                self.to_cursor_index = next_cursor;
                self.on_to_field_edited();
                // Token completion operates on the trailing recipient token, so
                // it only applies when editing at the end of the field.
                if self.to_cursor_index == self.to_field.len() {
                    self.maybe_autocomplete_at_prefixed_token();
                }
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
        self.to_cursor_index = self.to_field.len();
        self.to_completion = None;
        true
    }

    pub fn move_to_field_cursor_left(&mut self) {
        if self.focus != FocusField::To || self.to_cursor_index == 0 {
            return;
        }
        self.to_cursor_index =
            super::text_util::previous_char_boundary(self.to_field.as_str(), self.to_cursor_index);
        self.clear_to_completion();
    }

    pub fn move_to_field_cursor_right(&mut self) {
        if self.focus != FocusField::To || self.to_cursor_index >= self.to_field.len() {
            return;
        }
        self.to_cursor_index =
            super::text_util::next_char_boundary(self.to_field.as_str(), self.to_cursor_index);
        self.clear_to_completion();
    }

    pub fn move_to_field_cursor_home(&mut self) {
        if self.focus != FocusField::To {
            return;
        }
        self.to_cursor_index = 0;
        self.clear_to_completion();
    }

    pub fn move_to_field_cursor_end(&mut self) {
        if self.focus != FocusField::To {
            return;
        }
        self.to_cursor_index = self.to_field.len();
        self.clear_to_completion();
    }

    pub fn clear_to_field(&mut self) {
        if self.focus != FocusField::To {
            return;
        }
        self.to_field.clear();
        self.to_cursor_index = 0;
        self.clear_to_completion();
    }

    pub fn to_cursor_column(&self) -> usize {
        let clamped = self.to_cursor_index.min(self.to_field.len());
        self.to_field[..clamped].chars().count()
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
                completion_state.candidate_index = super::text_util::wrap_index(
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
        self.message_cursor_index = super::text_util::previous_char_boundary(
            self.message_field.as_str(),
            self.message_cursor_index,
        );
        self.message_cursor_preferred_column = None;
    }

    pub fn move_message_cursor_right(&mut self) {
        if self.focus != FocusField::Message
            || self.message_cursor_index >= self.message_field.len()
        {
            return;
        }
        self.message_cursor_index = super::text_util::next_char_boundary(
            self.message_field.as_str(),
            self.message_cursor_index,
        );
        self.message_cursor_preferred_column = None;
    }

    pub fn move_message_cursor_home(&mut self) {
        if self.focus != FocusField::Message {
            return;
        }
        let (line_start, _) = super::text_util::line_range_for_cursor(
            self.message_field.as_str(),
            self.message_cursor_index,
        );
        self.message_cursor_index = line_start;
        self.message_cursor_preferred_column = None;
    }

    pub fn move_message_cursor_end(&mut self) {
        if self.focus != FocusField::Message {
            return;
        }
        let (_, line_end) = super::text_util::line_range_for_cursor(
            self.message_field.as_str(),
            self.message_cursor_index,
        );
        self.message_cursor_index = line_end;
        self.message_cursor_preferred_column = None;
    }

    pub fn message_cursor_index(&self) -> usize {
        self.message_cursor_index
    }

    pub fn message_cursor_line_and_column(&self) -> (usize, usize) {
        super::text_util::line_and_column_for_index(
            self.message_field.as_str(),
            self.message_cursor_index,
        )
    }

    pub(in crate::tui::state) fn clear_compose_fields(&mut self) {
        self.to_field.clear();
        self.to_cursor_index = 0;
        self.message_field.clear();
        self.message_cursor_index = 0;
        self.message_cursor_preferred_column = None;
        self.focus = FocusField::To;
        self.clear_to_completion();
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
        self.to_cursor_index = self.to_field.len();
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
        let next_cursor = super::text_util::previous_char_boundary(
            self.message_field.as_str(),
            self.message_cursor_index,
        );
        self.message_field
            .replace_range(next_cursor..self.message_cursor_index, "");
        self.message_cursor_index = next_cursor;
        self.message_cursor_preferred_column = None;
    }

    fn move_message_cursor_vertical(&mut self, delta: isize) {
        let line_ranges = super::text_util::line_ranges(self.message_field.as_str());
        if line_ranges.is_empty() {
            return;
        }
        let (current_line, current_column) = super::text_util::line_and_column_for_index(
            self.message_field.as_str(),
            self.message_cursor_index,
        );
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
        self.message_cursor_index = super::text_util::cursor_index_for_line_column(
            self.message_field.as_str(),
            line_ranges[target_line],
            preferred_column,
        );
        self.message_cursor_preferred_column = Some(preferred_column);
    }
}
