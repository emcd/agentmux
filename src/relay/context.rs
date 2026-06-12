use std::{path::PathBuf, sync::mpsc};

use crate::{configuration::BundleConfiguration, envelope::PromptBatchSettings};

use super::identity::IdentityIntrospectRights;
use super::{DeliveryPayloadMode, RelayError, SendResult, delivery::QuiescenceOptions};

#[derive(Clone, Debug)]
pub(super) struct SendRequestContext {
    pub(super) request_id: Option<String>,
    pub(super) requester_session: String,
    pub(super) message: String,
    pub(super) targets: Vec<String>,
    pub(super) broadcast: bool,
    pub(super) quiet_window_ms: Option<u64>,
    pub(super) quiescence_timeout_ms: Option<u64>,
    pub(super) acp_turn_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub(super) struct PermissionDecisionRequestContext {
    pub(super) permission_request_id: String,
    pub(super) outcome: String,
    pub(super) option_id: Option<String>,
    pub(super) ui_session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct RequestPrincipal {
    pub(super) session_id: String,
    /// Verified `principal_id` of the requester, set only when the connection
    /// presented a store-backed credential; `None` for socket-trust sessions.
    pub(super) authenticated_identity: Option<String>,
    /// Introspection rights for an application principal, recorded at Hello;
    /// `None` for every other connection. Request dispatch gates
    /// `IdentityIntrospect` on this.
    pub(super) introspect_rights: Option<IdentityIntrospectRights>,
}

#[derive(Clone, Debug)]
pub(super) struct AsyncDeliveryTask {
    pub(super) bundle: BundleConfiguration,
    /// Home bundle of the sender. Differs from `bundle` (the delivery context)
    /// for cross-bundle fan-out, where `bundle` is the target's bundle; used to
    /// attribute and route the sender independently of the target's namespace.
    pub(super) sender_bundle_name: String,
    pub(super) sender: crate::configuration::BundleMember,
    /// Verified `principal_id` of the sender, carried into the delivered
    /// message envelope on the recipient side. `None` for socket-trust senders
    /// and for delivery paths that do not attribute a verified identity (e.g.
    /// raw input).
    pub(super) authenticated_identity: Option<String>,
    /// Canonical `session@namespace` ids of every recipient of this message
    /// across all delivery groups, the task's own target included. Envelope
    /// rendering derives co-recipient (Cc) identities from this list.
    pub(super) all_target_sessions: Vec<String>,
    pub(super) target_session: String,
    /// The target is a relay-wide (`@GLOBAL`) principal with no bundle-member
    /// transport configuration; delivery resolves it via the stream registry.
    /// Mirrors `RouteTarget.relay_wide` in `routing.rs`, which feeds it. Not a
    /// delivery-mechanism flag: stream-event delivery is decided by
    /// `should_route_to_ui`, a strict superset of this.
    pub(super) relay_wide_target: bool,
    pub(super) message: String,
    pub(super) message_id: String,
    pub(super) quiescence: QuiescenceOptions,
    pub(super) batch_settings: PromptBatchSettings,
    pub(super) runtime_directory: PathBuf,
    pub(super) completion_sender: Option<mpsc::Sender<Result<SendResult, RelayError>>>,
    pub(super) payload_mode: DeliveryPayloadMode,
    pub(super) append_enter: bool,
    pub(super) permission_decider_sessions: Vec<String>,
    pub(super) permission_max_pending: usize,
}
