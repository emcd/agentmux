//! The envelope a transport is handed and the outcome it reports back.
//!
//! [`DeliveryEnvelope`] is what the relay submits; [`DeliveryMessage`] is the
//! structured body a coder transport renders for its target; and
//! [`SingleDeliveryOutcome`] is the terminal result an [`OutcomeFuture`]
//! resolves to. Nothing here imports `crate::relay` — the relay maps the
//! resolved outcome onto its own `SendResult` at the collect site.

use serde_json::Value;
use tokio::sync::oneshot;

// Pane-envelope rendering helpers. Canonical home is the transport-safe
// `crate::envelope` module (it imports no relay internals), so coder transports
// can render their own pane text from the structured delivery message.
use crate::envelope::{AddressIdentity, EnvelopeRenderInput, render_envelope};
use crate::runtime::signals::shutdown_requested;
// Delivery/look wire vocabulary. Canonical home is the sibling `vocabulary`
// module (below `crate::relay` in dependency order), so the transport contract
// never depends on relay. The relay re-exports these from its own contract.
use crate::transports::vocabulary::SendOutcome;

/// A pending delivery outcome handed back by the non-blocking write methods
/// ([`Transport::mailw`](super::Transport::mailw),
/// [`Transport::raww`](super::Transport::raww)). It resolves to the terminal
/// [`SingleDeliveryOutcome`] once the transport's internal delivery task settles
/// the write; the sender half lives inside the transport's ordered channel item.
///
/// Carries the transport-side [`SingleDeliveryOutcome`], not the relay
/// `SendResult`: the transport contract never depends on `crate::relay`, so the
/// relay worker maps the resolved outcome onto its own `SendResult` at the
/// collect site.
pub type OutcomeFuture = oneshot::Receiver<SingleDeliveryOutcome>;

/// One structured message to deliver to a target, plus the per-write control
/// hints the transport's internal delivery task needs.
///
/// The relay populates [`message`](Self::message) with relay-authored attribution
/// after routing and authorization; the transport renders its own representation
/// from those fields (coder transports render pane-envelope text, UI builds a
/// stream event) and never infers or mutates attribution. The remaining fields
/// are per-write transport control, not message content.
#[derive(Clone, Debug)]
pub struct DeliveryEnvelope {
    /// Correlation id echoed back in the [`SingleDeliveryOutcome`].
    pub message_id: String,
    /// Structured, transport-neutral message data. The receiving transport
    /// renders the representation it owns from these fields.
    pub message: DeliveryMessage,
    /// Whether to submit (append Enter) after writing the rendered text.
    pub append_enter: bool,
    /// Sessions authorized to decide choices raised during this envelope's
    /// delivery, threaded to
    /// [`ChoiceToMake::decider_sessions`](super::ChoiceToMake::decider_sessions).
    pub choice_decider_sessions: Vec<String>,
    /// True when this envelope carries a terminal-outcome receipt (a
    /// relay/system-originated notice back to the original sender for a
    /// non-delivered outcome). Carried on the envelope so per-transport
    /// rendering polish (e.g. ACP's flush-barrier behavior) can branch on it
    /// without re-deriving from the message body. The relay's delivery
    /// mechanics are receipt-agnostic; this flag is a per-transport hint,
    /// not a dispatch concern. `false` for ordinary peer messages.
    pub is_receipt: bool,
}

/// Structured, transport-neutral message data sufficient for any transport to
/// render its own representation without importing `crate::relay` or parsing
/// already-rendered text. The relay authors every field; transports treat them
/// as read-only input.
///
/// Each party is carried as an [`AddressIdentity`] directly: coder transports
/// render the decorating pane-header form via `render_address`, while
/// machine-consumed event fields use the bare
/// [`AddressIdentity::canonical_session_id`] form.
#[derive(Clone, Debug)]
pub struct DeliveryMessage {
    /// The message body text.
    pub body: String,
    /// RFC 3339 creation timestamp, rendered into the `Date` header.
    pub created_at: String,
    /// The routing namespace qualifying canonical `session@namespace` ids
    /// (a session bundle, or a relay-wide namespace such as `GLOBAL`).
    pub namespace: String,
    /// Canonical sender identity.
    pub sender: AddressIdentity,
    /// Canonical target identity.
    pub target: AddressIdentity,
    /// Canonical co-recipient identities (the full target set minus this
    /// envelope's own recipient), including co-recipients in other namespaces.
    pub cc: Vec<AddressIdentity>,
    /// The sender's verified `principal_id`, when present; `None` for
    /// socket-trust senders.
    pub authenticated_identity: Option<String>,
    /// Origin principal a peer relay forwarded this message on behalf of, carried
    /// uninterpreted alongside `authenticated_identity` (the peer relay). `None`
    /// for local delivery and non-relay senders.
    pub on_behalf_of: Option<String>,
}

impl DeliveryMessage {
    /// Renders this message as RFC 822/MIME pane-envelope text. Coder transports
    /// (Tmux/ACP) call this before writing to the harness; UI does not render
    /// pane text. `message_id` is the owning envelope's correlation id, which
    /// seeds the MIME boundary and `Message-Id` header.
    #[must_use]
    pub fn render_pane_envelope(&self, message_id: &str) -> String {
        render_envelope(&EnvelopeRenderInput {
            message_id: message_id.to_string(),
            created_at: self.created_at.clone(),
            from: self.sender.clone(),
            to: vec![self.target.clone()],
            cc: self.cc.clone(),
            subject: None,
            body: self.body.clone(),
        })
    }
}

/// The transport-level outcome for one delivered envelope. Structurally mirrors
/// the relay `SendResult`; kept distinct so the transport vocabulary can evolve
/// independently of the relay wire contract.
#[derive(Clone, Debug)]
pub struct SingleDeliveryOutcome {
    pub target_session: String,
    pub message_id: String,
    pub outcome: SendOutcome,
    pub reason_code: Option<String>,
    pub reason: Option<String>,
    pub details: Option<Value>,
}

/// The outcome for a member whose generation stopped before its submission was
/// attempted.
///
/// The outcome is `not_submitted` and is not the caller's to vary. A member the
/// transport still held when it was told to stop was never partitioned and never
/// written, so nothing reached the target — which is what `not_submitted`
/// asserts and what makes it provable here rather than inferred. A transport
/// resolving such a member `dropped_on_shutdown` instead lets a lifecycle event
/// choose an outcome, which is precisely what the guard's evidence order exists
/// to prevent: shutdown and fencing are triggers, and only members the relay
/// still holds as `Pending` carry the shutdown spelling.
///
/// This is the outcome the **transport** reports, which is what a sender is told
/// only for an *unbound* member. Envelope members are unbound here, because a
/// transport that reports its own partition declares inside its write. A **raw**
/// member is not: the relay declares its singleton unit before calling `raww`,
/// so a raw write stopped in a transport's channel is already bound and the
/// relay reconciles this result to its unit's record — no evidence having been
/// recorded, `submission_unknown`. That is the evidence order working, not a gap
/// to route around here.
///
/// The **cause** is reported separately, in the reason code, because a transport
/// cannot tell why it was stopped from its own stop signal: the relay's shutdown
/// drain fences the generation as its cooperative step and tears the transport
/// down only after the verdict, so the same signal serves both lifecycles. The
/// process-wide shutdown state is therefore asked directly rather than inferred
/// from which flag was set. It is diagnostic and nothing downstream branches on
/// it, so a watchdog fence landing after shutdown was requested reporting the
/// shutdown cause costs a reader nothing.
#[must_use]
pub(crate) fn stopped_before_submission_outcome(
    target_session: String,
    message_id: String,
) -> SingleDeliveryOutcome {
    let (reason_code, reason) = if shutdown_requested() {
        (
            "relay_shutdown",
            "relay shutdown stopped the delivery generation before submission",
        )
    } else {
        (
            "generation_fenced",
            "the delivery generation was fenced before submission",
        )
    };
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::NotSubmitted,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.to_string()),
        details: None,
    }
}
