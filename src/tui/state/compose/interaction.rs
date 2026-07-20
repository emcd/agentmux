use crate::{
    relay::{ListedSessionTransport, LookSnapshotPayload, RelayRequest, RelayResponse},
    runtime::error::RuntimeError,
};

use super::{AppState, LookSnapshotFormat, ScreenMode, map_relay_error};

impl AppState {
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

    pub fn enter_interaction_mode(&mut self) {
        self.mode = ScreenMode::Interaction;
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
            offset: None,
        })?;

        match response {
            RelayResponse::Look {
                target_session,
                captured_at,
                snapshot,
                ..
            } => {
                self.last_selected_recipient = Some(target_session.clone());
                self.set_interaction_target(target_session.clone());
                self.store_look_snapshot(captured_at, snapshot);
                self.close_picker();
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

    /// Re-captures the look snapshot for the active interaction target, replacing
    /// the overlay buffer in place. The interaction target is unchanged, so the
    /// operator's scroll position is preserved. With no target selected there is
    /// nothing to refresh. Errors are propagated so the caller (entering
    /// Interaction on an existing target) can surface a relay failure.
    pub fn refresh_look_snapshot(&mut self) -> Result<(), RuntimeError> {
        let Some(target) = self.look_target.clone() else {
            return Ok(());
        };

        let response = self.request_relay(&RelayRequest::Look {
            requester_session: self.sender_session.clone(),
            target_session: target,
            lines: self.look_lines.map(|value| value as usize),
            offset: None,
        })?;

        match response {
            RelayResponse::Look {
                captured_at,
                snapshot,
                ..
            } => {
                self.store_look_snapshot(captured_at, snapshot);
                Ok(())
            }
            RelayResponse::Error { error } => Err(map_relay_error(error)),
            other => Err(RuntimeError::validation(
                "internal_unexpected_failure",
                format!("relay returned unexpected response variant: {other:?}"),
            )),
        }
    }

    /// Stores a captured look payload into the overlay buffer fields, leaving the
    /// interaction target and overlay scroll position untouched.
    fn store_look_snapshot(&mut self, captured_at: String, snapshot: LookSnapshotPayload) {
        let (look_snapshot_format, look_snapshot_lines, look_snapshot_entries) =
            overlay_snapshot_from_payload(snapshot);
        self.look_captured_at = Some(captured_at);
        self.look_snapshot_format = Some(look_snapshot_format);
        self.look_snapshot_lines = look_snapshot_lines;
        self.look_snapshot_entries = look_snapshot_entries;
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
            requester_session: self.sender_session.clone(),
            target_session: target.clone(),
            text,
            no_enter: false,
            on_behalf_of: None,
        })?;

        match response {
            RelayResponse::Raww {
                status,
                target_session,
                transport,
                request_id: _,
                message_id,
                ..
            } => {
                let transport_label = render_transport_label(transport);
                let message_id_label = message_id.as_deref().unwrap_or("-");

                // Raww delivery is asynchronous: status is always `queued` and
                // the terminal outcome arrives via a later delivery_outcome
                // stream event. Enroll the message_id into pending-delivery
                // tracking so the verb-agnostic delivery_outcome consumer
                // (history.rs) closes it out, mirroring how chat send results
                // are recorded. Guard against a terminal outcome that already
                // raced ahead of this queued acknowledgement.
                if let Some(message_id) = message_id.as_deref()
                    && !self.terminal_delivery_message_ids.contains(message_id)
                {
                    self.pending_delivery_ids.insert(message_id.to_string());
                }

                self.push_status(
                    None,
                    format!("write {status} target={target_session} transport={transport_label}"),
                );
                self.push_event(format!(
                    "write target={target_session} status={status} transport={transport_label} message_id={message_id_label} pending={}",
                    self.pending_deliveries_count()
                ));
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
            self.look_choice_request_index = 0;
            self.look_choice_option_index = 0;
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
        let next_cursor = super::text_util::previous_char_boundary(
            self.raww_draft.as_str(),
            self.raww_cursor_index,
        );
        self.raww_draft
            .replace_range(next_cursor..self.raww_cursor_index, "");
        self.raww_cursor_index = next_cursor;
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_left(&mut self) {
        if self.raww_cursor_index == 0 {
            return;
        }
        self.raww_cursor_index = super::text_util::previous_char_boundary(
            self.raww_draft.as_str(),
            self.raww_cursor_index,
        );
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_right(&mut self) {
        if self.raww_cursor_index >= self.raww_draft.len() {
            return;
        }
        self.raww_cursor_index =
            super::text_util::next_char_boundary(self.raww_draft.as_str(), self.raww_cursor_index);
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_home(&mut self) {
        let (line_start, _) = super::text_util::line_range_for_cursor(
            self.raww_draft.as_str(),
            self.raww_cursor_index,
        );
        self.raww_cursor_index = line_start;
        self.raww_cursor_preferred_column = None;
    }

    pub fn move_raww_cursor_end(&mut self) {
        let (_, line_end) = super::text_util::line_range_for_cursor(
            self.raww_draft.as_str(),
            self.raww_cursor_index,
        );
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
        let line_ranges = super::text_util::line_ranges(self.raww_draft.as_str());
        if line_ranges.is_empty() {
            return;
        }
        let (current_line, current_column) = super::text_util::line_and_column_for_index(
            self.raww_draft.as_str(),
            self.raww_cursor_index,
        );
        let target_line = if delta.is_negative() {
            current_line.saturating_sub(delta.unsigned_abs())
        } else {
            (current_line + delta as usize).min(line_ranges.len().saturating_sub(1))
        };
        if target_line == current_line {
            return;
        }
        let preferred_column = self.raww_cursor_preferred_column.unwrap_or(current_column);
        self.raww_cursor_index = super::text_util::cursor_index_for_line_column(
            self.raww_draft.as_str(),
            line_ranges[target_line],
            preferred_column,
        );
        self.raww_cursor_preferred_column = Some(preferred_column);
    }

    pub fn raww_cursor_line_and_column(&self) -> (usize, usize) {
        super::text_util::line_and_column_for_index(
            self.raww_draft.as_str(),
            self.raww_cursor_index,
        )
    }

    pub fn clear_raww_draft(&mut self) {
        self.raww_draft.clear();
        self.raww_cursor_index = 0;
        self.raww_cursor_preferred_column = None;
    }

    pub(crate) fn interaction_raww_region_visible(&self) -> bool {
        !self.raww_draft.is_empty() || self.look_pending_choices().is_empty()
    }
}

fn overlay_snapshot_from_payload(
    snapshot: LookSnapshotPayload,
) -> (
    LookSnapshotFormat,
    Vec<String>,
    Vec<crate::transports::StructuredEntry>,
) {
    match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => {
            (LookSnapshotFormat::Lines, snapshot_lines, Vec::new())
        }
        LookSnapshotPayload::StructuredEntriesV1 {
            snapshot_entries, ..
        } => (
            LookSnapshotFormat::StructuredEntriesV1,
            Vec::new(),
            snapshot_entries,
        ),
    }
}

fn render_transport_label(transport: ListedSessionTransport) -> &'static str {
    match transport {
        ListedSessionTransport::Tmux => "tmux",
        ListedSessionTransport::Acp => "acp",
        ListedSessionTransport::Pty => "pty",
        ListedSessionTransport::Ui => "ui",
        ListedSessionTransport::Pubsub => "pubsub",
    }
}
