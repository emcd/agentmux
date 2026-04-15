use crate::{
    relay::{RelayRequest, RelayResponse},
    runtime::error::RuntimeError,
};

use super::{AppState, Recipient, map_relay_error, map_relay_request_failure};

impl AppState {
    pub fn refresh_recipients(&mut self) -> Result<(), RuntimeError> {
        let response = self.request_relay(&RelayRequest::List {
            sender_session: Some(self.sender_session.clone()),
        })?;
        match response {
            RelayResponse::List { bundle, .. } => {
                let recipients = bundle
                    .sessions
                    .into_iter()
                    .map(|session| Recipient {
                        session_name: session.id,
                        display_name: session.name,
                    })
                    .collect::<Vec<_>>();
                self.recipients = recipients;
                if self.recipients.is_empty() {
                    self.recipients_state.select(None);
                    self.picker_state.select(None);
                } else {
                    self.ensure_recipient_selection();
                }
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

    pub fn poll_relay_events(&mut self) {
        match self.relay_stream.poll_events() {
            Ok(events) => {
                if self.relay_stream_poll_error_reported {
                    self.push_status(None, "relay stream reconnected");
                }
                self.relay_stream_poll_error_reported = false;
                self.record_stream_events(&events);
            }
            Err(source) => {
                if self.relay_stream_poll_error_reported {
                    return;
                }
                self.relay_stream_poll_error_reported = true;
                self.push_runtime_error(map_relay_request_failure(&self.relay_socket, source));
            }
        }
    }

    fn ensure_recipient_selection(&mut self) {
        if self.recipients.is_empty() {
            self.recipients_state.select(None);
            self.picker_state.select(None);
            return;
        }
        let index = self
            .recipients_state
            .selected()
            .filter(|index| *index < self.recipients.len())
            .unwrap_or(0);
        self.recipients_state.select(Some(index));
        self.picker_state.select(Some(index));
    }

    pub(super) fn request_relay(
        &mut self,
        request: &RelayRequest,
    ) -> Result<RelayResponse, RuntimeError> {
        match self.relay_stream.request_with_events(request) {
            Ok((response, events)) => {
                self.record_stream_events(&events);
                Ok(response)
            }
            Err(source) => Err(map_relay_request_failure(&self.relay_socket, source)),
        }
    }
}
