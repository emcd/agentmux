use std::collections::{HashSet, VecDeque};

use crate::{
    relay::{RelayRequest, RelayResponse, RelayStreamSession},
    runtime::error::RuntimeError,
};

use super::{AppState, BundleStatusDisplay, Recipient, map_relay_error, map_relay_request_failure};

impl AppState {
    pub fn refresh_recipients(&mut self) -> Result<(), RuntimeError> {
        let response = self.request_relay(&RelayRequest::List {
            requester_session: Some(self.sender_session.clone()),
        })?;
        match response {
            RelayResponse::List { bundle, .. } => {
                self.bundle_status = Some(BundleStatusDisplay::from_listed_bundle(&bundle));
                let recipients = bundle
                    .principals
                    .into_iter()
                    .map(|session| Recipient {
                        session_name: session.id,
                        display_name: session.name,
                        ready: session.ready,
                    })
                    .collect::<Vec<_>>();
                self.recipients = recipients;
                self.apply_recipient_list_update();
                self.push_status(
                    None,
                    format!(
                        "Loaded {} recipients for bundle {}.",
                        self.recipients.len(),
                        self.bundle_name
                    ),
                );
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

    pub fn poll_relay_events(&mut self) -> bool {
        match self.relay_stream.poll_events() {
            Ok(events) => {
                let reconnected = self.relay_stream_poll_error_reported;
                if reconnected {
                    self.push_status(None, "relay stream reconnected");
                }
                self.relay_stream_poll_error_reported = false;
                let had_events = !events.is_empty();
                self.record_stream_events(&events);
                reconnected || had_events
            }
            Err(source) => {
                if self.relay_stream_poll_error_reported {
                    return false;
                }
                self.relay_stream_poll_error_reported = true;
                self.push_runtime_error(map_relay_request_failure(&self.relay_socket, source));
                true
            }
        }
    }

    pub fn switch_to_selected_bundle(&mut self) -> Result<(), RuntimeError> {
        let Some(index) = self.bundle_picker_state.selected() else {
            return Err(RuntimeError::validation(
                "validation_unknown_target",
                "bundle switch requires a selected bundle in picker",
            ));
        };
        let Some(target_bundle) = self.available_bundles.get(index).cloned() else {
            return Err(RuntimeError::validation(
                "validation_unknown_target",
                "bundle picker selection is out of range",
            ));
        };
        if target_bundle == self.bundle_name {
            self.bundle_picker_open = false;
            return Ok(());
        }
        self.bundle_name = target_bundle.clone();
        self.relay_stream = RelayStreamSession::new(
            self.relay_socket.clone(),
            target_bundle.clone(),
            self.sender_session.clone(),
        );
        self.reset_bundle_scoped_state();
        self.bundle_picker_open = false;
        self.push_status(None, format!("Switched to bundle {target_bundle}."));
        self.refresh_recipients()
    }

    fn reset_bundle_scoped_state(&mut self) {
        self.recipients.clear();
        self.last_selected_recipient = None;
        self.bundle_status = None;
        self.picker_state.select(None);
        self.look_target = None;
        self.look_captured_at = None;
        self.look_snapshot_format = None;
        self.look_snapshot_lines.clear();
        self.look_snapshot_entries.clear();
        self.look_overlay_scroll = 0;
        self.look_permission_request_index = 0;
        self.look_permission_option_index = 0;
        self.pending_permissions.clear();
        self.pending_permissions_state.select(None);
        self.chat_history.clear();
        self.event_history.clear();
        self.pending_delivery_ids = HashSet::new();
        self.terminal_delivery_message_ids = HashSet::new();
        self.terminal_delivery_message_order = VecDeque::new();
        self.seen_incoming_message_ids = HashSet::new();
        self.seen_incoming_message_order = VecDeque::new();
        self.seen_delivery_outcome_ids = HashSet::new();
        self.seen_delivery_outcome_order = VecDeque::new();
        self.clear_raww_draft();
        self.relay_stream_poll_error_reported = false;
    }

    pub(super) fn request_relay(
        &mut self,
        request: &RelayRequest,
    ) -> Result<RelayResponse, RuntimeError> {
        // The relay client resolves the wire namespace from the request type:
        // suffix-routed Send/Raww carry none, namespace-selected ops fall back to
        // the bound (browsing) bundle. Callers dispatch uniformly (issues/tui/11).
        match self.relay_stream.request_with_events(request) {
            Ok((response, events)) => {
                self.record_stream_events(&events);
                Ok(response)
            }
            Err(source) => Err(map_relay_request_failure(&self.relay_socket, source)),
        }
    }
}
