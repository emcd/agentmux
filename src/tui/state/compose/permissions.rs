use crate::{
    relay::{RelayRequest, RelayResponse},
    runtime::error::RuntimeError,
};

use super::{AppState, PendingPermissionOption, map_relay_error};

impl AppState {
    pub fn move_look_permission_request_selection(&mut self, delta: isize) {
        let entries = self.look_pending_permissions();
        if entries.is_empty() {
            self.look_permission_request_index = 0;
            self.look_permission_option_index = 0;
            return;
        }
        self.look_permission_request_index =
            super::text_util::wrap_index(self.look_permission_request_index, delta, entries.len());
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
        self.look_permission_option_index = super::text_util::wrap_index(
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

    pub(in crate::tui::state) fn ensure_pending_permission_selection(&mut self) {
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

    pub(crate) fn look_pending_permissions(&self) -> Vec<&super::PendingPermissionEntry> {
        let Some(look_target) = self.look_target.as_deref() else {
            return Vec::new();
        };
        self.pending_permissions
            .iter()
            .filter(|entry| entry.target_session.as_deref() == Some(look_target))
            .collect::<Vec<_>>()
    }

    pub(in crate::tui::state) fn selected_look_permission(
        &self,
    ) -> Option<&super::PendingPermissionEntry> {
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

    pub(in crate::tui::state) fn selected_look_permission_option(
        &self,
    ) -> Option<&PendingPermissionOption> {
        let entry = self.selected_look_permission()?;
        if entry.options.is_empty() {
            return None;
        }
        entry.options.get(
            self.look_permission_option_index
                .min(entry.options.len().saturating_sub(1)),
        )
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
}
