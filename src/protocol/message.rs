//! The structured message a target receives, and the envelope carrying it.
//!
//! Both sit in this boundary because both call directions name them: the relay
//! admits an envelope into a target's mailbox, and a transport reads one back out
//! of a peek and renders its own representation from it. Neither has to import
//! the other to say what a delivery *is*.

use crate::envelope::{AddressIdentity, EnvelopeRenderInput, render_envelope};

/// One structured message to deliver to a target, plus the per-write control
/// hints the transport's delivery loop needs.
///
/// The relay populates [`message`](Self::message) with relay-authored attribution
/// after routing and authorization; the transport renders its own representation
/// from those fields (coder transports render pane-envelope text, UI builds a
/// stream event) and never infers or mutates attribution. The remaining fields
/// are per-write transport control, not message content.
#[derive(Clone, Debug)]
pub struct DeliveryEnvelope {
    /// Correlation id echoed back in the delivery outcome.
    pub message_id: String,
    /// Structured, transport-neutral message data. The receiving transport
    /// renders the representation it owns from these fields.
    pub message: DeliveryMessage,
    /// Whether to submit (append Enter) after writing the rendered text.
    pub append_enter: bool,
    /// Sessions authorized to decide choices raised during this envelope's
    /// delivery, threaded to the transport's `ChoiceToMake::decider_sessions`.
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
/// render its own representation without importing relay internals or parsing
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
