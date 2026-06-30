use std::collections::{BTreeSet, HashSet, VecDeque};

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
                        self.namespace
                    ),
                );
                self.refresh_cross_bundle_candidates();
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

    /// Commits the bundle-column selection in the unified picker. Selecting the
    /// active bundle is a no-op that simply hands focus to the session column;
    /// selecting a different bundle switches the active context, re-enumerates
    /// its sessions, and keeps the picker open focused on those sessions so the
    /// operator can pick one in the same window.
    pub fn commit_selected_picker_bundle(&mut self) -> Result<(), RuntimeError> {
        let Some(target_namespace) = self.selected_picker_bundle() else {
            return Err(RuntimeError::validation(
                "validation_unknown_target",
                "bundle switch requires a selected bundle in picker",
            ));
        };
        if target_namespace == self.namespace {
            self.focus_picker_session_column();
            return Ok(());
        }
        self.namespace = target_namespace.clone();
        self.relay_stream = RelayStreamSession::new(
            self.relay_socket.clone(),
            target_namespace.clone(),
            self.sender_session.clone(),
        );
        self.reset_bundle_scoped_state();
        self.focus_picker_session_column();
        self.push_status(None, format!("Switched to bundle {target_namespace}."));
        self.refresh_recipients()
    }

    fn reset_bundle_scoped_state(&mut self) {
        self.recipients.clear();
        self.last_selected_recipient = None;
        self.bundle_status = None;
        self.picker_session_state.select(None);
        self.look_target = None;
        self.look_captured_at = None;
        self.look_snapshot_format = None;
        self.look_snapshot_lines.clear();
        self.look_snapshot_entries.clear();
        self.look_overlay_scroll = 0;
        self.look_choice_request_index = 0;
        self.look_choice_option_index = 0;
        self.pending_choices.clear();
        self.pending_choices_state.select(None);
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

    /// Fans out a `List` over every available bundle other than the active
    /// `namespace` to assemble relay-wide To-field completion candidates as
    /// canonical `session@bundle` principal ids. Runs eagerly on the same
    /// cadence as [`refresh_recipients`] so cross-bundle candidates stay fresh.
    ///
    /// The active bundle is skipped because its canonical ids already arrive via
    /// `self.recipients`. Per-bundle failures degrade silently: a bundle the
    /// operator cannot enumerate (relay answers `authorization_forbidden` for
    /// insufficient `all` scope) or a transient transport error simply drops out
    /// of the candidate pool rather than failing the whole refresh — the home
    /// recipients are already loaded and authoritative.
    fn refresh_cross_bundle_candidates(&mut self) {
        let other_bundles = self
            .available_bundles
            .iter()
            .filter(|bundle| bundle.as_str() != self.namespace.as_str())
            .cloned()
            .collect::<Vec<_>>();
        let mut candidates = BTreeSet::<String>::new();
        for bundle in other_bundles {
            let request = RelayRequest::List {
                requester_session: Some(self.sender_session.clone()),
            };
            let (response, events) = match self
                .relay_stream
                .request_with_namespace_and_events(&request, Some(bundle.as_str()))
            {
                Ok(pair) => pair,
                Err(_) => continue,
            };
            self.record_stream_events(&events);
            if let RelayResponse::List { bundle: listed, .. } = response {
                for principal in listed.principals {
                    candidates.insert(principal.id);
                }
            }
        }
        self.cross_bundle_candidates = candidates.into_iter().collect();
    }

    pub(super) fn request_relay(
        &mut self,
        request: &RelayRequest,
    ) -> Result<RelayResponse, RuntimeError> {
        // The relay client resolves the wire namespace from the request type:
        // suffix-routed Send/Raww carry none, namespace-selected ops fall back to
        // the bound (browsing) bundle. Callers dispatch uniformly.
        match self.relay_stream.request_with_events(request) {
            Ok((response, events)) => {
                self.record_stream_events(&events);
                Ok(response)
            }
            Err(source) => Err(map_relay_request_failure(&self.relay_socket, source)),
        }
    }
}
