use crate::{
    relay::{RelayRequest, RelayResponse},
    runtime::error::RuntimeError,
};

use super::{AppState, PendingChoiceOption, map_relay_error};

impl AppState {
    pub fn move_look_choice_request_selection(&mut self, delta: isize) {
        let entries = self.look_pending_choices();
        if entries.is_empty() {
            self.look_choice_request_index = 0;
            self.look_choice_option_index = 0;
            return;
        }
        self.look_choice_request_index =
            super::text_util::wrap_index(self.look_choice_request_index, delta, entries.len());
        self.look_choice_option_index = 0;
    }

    pub fn move_look_choice_option_selection(&mut self, delta: isize) {
        let Some(entry) = self.selected_look_choice() else {
            self.look_choice_option_index = 0;
            return;
        };
        if entry.options.is_empty() {
            self.look_choice_option_index = 0;
            return;
        }
        self.look_choice_option_index =
            super::text_util::wrap_index(self.look_choice_option_index, delta, entry.options.len());
    }

    pub fn resolve_selected_look_choice_selected(&mut self) -> Result<(), RuntimeError> {
        let Some(entry) = self.selected_look_choice() else {
            return Err(RuntimeError::validation(
                "validation_unknown_choice_request",
                "selected outcome requires a pending choice request for the current look target",
            ));
        };
        let Some(option) = self.selected_look_choice_option() else {
            return Err(RuntimeError::validation(
                "validation_invalid_params",
                "selected outcome requires explicit option selection",
            ));
        };
        self.submit_choice_decision(
            entry.choice_request_id.clone(),
            "selected",
            Some(option.option_id.clone()),
        )
    }

    pub fn resolve_selected_look_choice_cancelled(&mut self) -> Result<(), RuntimeError> {
        let choice_request_id = self
            .selected_look_choice()
            .map(|entry| entry.choice_request_id.clone())
            .ok_or_else(|| {
                RuntimeError::validation(
                    "validation_unknown_choice_request",
                    "cancelled outcome requires a pending choice request for the current look target",
                )
            })?;
        self.submit_choice_decision(choice_request_id, "cancelled", None)
    }

    pub(in crate::tui::state) fn ensure_pending_choice_selection(&mut self) {
        if self.pending_choices.is_empty() {
            self.pending_choices_state.select(None);
            return;
        }
        let selected = self
            .pending_choices_state
            .selected()
            .filter(|index| *index < self.pending_choices.len())
            .unwrap_or(0);
        self.pending_choices_state.select(Some(selected));
    }

    pub(crate) fn look_pending_choices(&self) -> Vec<&super::PendingChoiceEntry> {
        let Some(look_target) = self.look_target.as_deref() else {
            return Vec::new();
        };
        self.pending_choices
            .iter()
            .filter(|entry| entry.target_session.as_deref() == Some(look_target))
            .collect::<Vec<_>>()
    }

    pub(in crate::tui::state) fn selected_look_choice(&self) -> Option<&super::PendingChoiceEntry> {
        let entries = self.look_pending_choices();
        if entries.is_empty() {
            return None;
        }
        entries
            .get(
                self.look_choice_request_index
                    .min(entries.len().saturating_sub(1)),
            )
            .copied()
    }

    pub(in crate::tui::state) fn selected_look_choice_option(
        &self,
    ) -> Option<&PendingChoiceOption> {
        let entry = self.selected_look_choice()?;
        if entry.options.is_empty() {
            return None;
        }
        entry.options.get(
            self.look_choice_option_index
                .min(entry.options.len().saturating_sub(1)),
        )
    }

    fn submit_choice_decision(
        &mut self,
        choice_request_id: String,
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
        let request = RelayRequest::ChoicesPick {
            choice_request_id: choice_request_id.clone(),
            outcome: outcome.to_string(),
            option_id,
        };
        match self.request_relay(&request)? {
            RelayResponse::ChoicesPick {
                status,
                choice_request_id,
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
                            "choice decision status={status} id={choice_request_id} outcome={outcome} reason_code={reason_code_label} reason={reason}"
                        ),
                    );
                } else {
                    self.push_status(
                        None,
                        format!(
                            "choice decision status={status} id={choice_request_id} outcome={outcome} reason_code={reason_code_label}"
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
}
