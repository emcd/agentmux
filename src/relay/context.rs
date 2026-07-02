use std::path::PathBuf;

use crate::configuration::BundleConfiguration;

use super::identity::IdentityIntrospectRights;
use super::{DeliveryPayloadMode, delivery::QuiescenceOptions};

#[derive(Clone, Debug)]
pub(super) struct SendRequestContext {
    pub(super) request_id: Option<String>,
    pub(super) requester_session: String,
    pub(super) message: String,
    pub(super) targets: Vec<String>,
    pub(super) broadcast: bool,
    pub(super) quiet_window_ms: Option<u64>,
    /// Origin attribution carried on a peer-forwarded request. Honored only when
    /// the authenticated requester is a relay principal (ingress); ignored for a
    /// regular session so a non-relay requester cannot self-assert it.
    pub(super) on_behalf_of: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct ChoiceDecisionRequestContext {
    pub(super) choice_request_id: String,
    pub(super) outcome: String,
    pub(super) option_id: Option<String>,
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
    /// Cross-relay ingress scope for a peer relay (`<id>@RELAY`) principal,
    /// recorded at Hello; `None` for every other connection. A forwarded
    /// `Send`/`Raww` from a peer relay is gated to this scope (deny-by-default
    /// when absent).
    pub(super) ingress_scope: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct AsyncDeliveryTask {
    pub(super) bundle: BundleConfiguration,
    /// Home namespace of the sender. Differs from `bundle` (the delivery context)
    /// for cross-bundle fan-out, where `bundle` is the target's bundle; used to
    /// attribute and route the sender independently of the target's namespace.
    pub(super) sender_namespace: String,
    pub(super) sender: crate::configuration::BundleMember,
    /// Verified `principal_id` of the sender, carried into the delivered
    /// message envelope on the recipient side. `None` for socket-trust senders
    /// and for delivery paths that do not attribute a verified identity (e.g.
    /// raw input).
    pub(super) authenticated_identity: Option<String>,
    /// Origin principal a peer relay forwarded this delivery on behalf of, carried
    /// uninterpreted into the delivered envelope alongside `authenticated_identity`
    /// (which names the peer relay). `None` for local delivery, socket-trust
    /// senders, and raw input (which has no attribution envelope).
    pub(super) on_behalf_of: Option<String>,
    /// Canonical `session@namespace` ids of every recipient of this message
    /// across all delivery groups, the task's own target included. Envelope
    /// rendering derives co-recipient (Cc) identities from this list.
    pub(super) all_target_sessions: Vec<String>,
    pub(super) target_session: String,
    pub(super) message: String,
    pub(super) message_id: String,
    pub(super) quiescence: QuiescenceOptions,
    pub(super) runtime_directory: PathBuf,
    pub(super) payload_mode: DeliveryPayloadMode,
    pub(super) append_enter: bool,
    pub(super) choice_decider_sessions: Vec<String>,
}
