use std::path::PathBuf;

use crate::configuration::{BundleConfiguration, BundleMember};

use super::DeliveryPayloadMode;
use super::identity::IdentityIntrospectRights;

#[derive(Clone, Debug)]
pub(super) struct SendRequestContext {
    pub(super) request_id: Option<String>,
    pub(super) requester_session: String,
    pub(super) message: String,
    pub(super) targets: Vec<String>,
    pub(super) broadcast: bool,
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
    /// `principal_id` the requester's connection was admitted under, set for
    /// every accepted Hello whether or not a credential backed it. Cross-relay
    /// forwarding stamps `on_behalf_of` from this; local attribution does not
    /// read it.
    pub(super) admitted_identity: Option<String>,
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
    pub(super) runtime_directory: PathBuf,
    pub(super) payload_mode: DeliveryPayloadMode,
    pub(super) append_enter: bool,
    pub(super) choice_decider_sessions: Vec<String>,
    /// True when this task *is* a terminal-outcome receipt (a relay-originated
    /// notice delivered back to an original sender). It gates non-recursion at
    /// the single terminal-resolution site: a receipt's own terminal outcome
    /// never spawns a receipt of its own.
    pub(super) is_receipt: bool,
    /// True when this task holds an admission reservation.
    ///
    /// The terminal transition removes its ledger entry — it must, or the ledger
    /// would grow by one record per message the relay ever delivered — so after
    /// the fact an absent reservation is ambiguous: either nothing was ever
    /// admitted for this task, or another resolver already won and cleaned up.
    /// This flag is what distinguishes them, and it is what keeps two competing
    /// resolvers from each reporting an outcome for one accepted member.
    ///
    /// Set by the request-boundary paths that admit. Relay-originated work that
    /// bypasses admission — terminal-outcome receipts above all — leaves it
    /// false and therefore stays reportable.
    pub(super) admitted: bool,
    /// Return route to the original sender for a terminal-outcome receipt: the
    /// sender's real bundle member (its true transport) and runtime directory,
    /// resolved from the sender's HOME bundle at send time. `None` for a
    /// non-bundle sender (`GLOBAL`/`RELAY`, served instead by the UI
    /// `delivery_outcome` stream frame), for raw-input delivery, and for
    /// receipt tasks themselves. Built from the sender's context, never the
    /// target's, so a cross-bundle receipt routes to the sender's own
    /// transport rather than misrouting to the target's.
    pub(super) sender_return_route: Option<SenderReturnRoute>,
}

/// The sender's own delivery context, carried on a delivery task so a
/// non-delivered terminal outcome can be routed back to the sender as a
/// terminal-outcome receipt. Holds the sender's *real* bundle member — with its
/// true [`TargetConfiguration`](crate::configuration::TargetConfiguration), not
/// the synthetic Tmux stub `SenderIdentity::to_bundle_member` produces — so the
/// receipt renders and delivers through the sender's actual transport.
#[derive(Clone, Debug)]
pub(super) struct SenderReturnRoute {
    pub(super) member: BundleMember,
    pub(super) runtime_directory: PathBuf,
}
