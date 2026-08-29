//! Transport interface contract for the relay delivery subsystem.
//!
//! The relay delivery worker dispatches every agent delivery operation through
//! the [`Transport`] trait. Concrete transports (ACP, Tmux, UI) each implement
//! the trait in their own module; the relay selects between them via the
//! [`TransportImpl`] enum, which delegates by `match` with no dynamic
//! allocation. Promoting UI (and the forward-declared Pubsub) to first-class
//! transports retires the relay's former `Acp/Tmux/Ui/Pubsub` routing fork.
//!
//! ## Write boundary: non-blocking, future-resolved
//!
//! The write methods ([`Transport::mailw`] for relay-framed envelopes,
//! [`Transport::raww`] for raw input) do not block. Each enqueues the write onto
//! the transport's own internal ordered channel and returns an [`OutcomeFuture`]
//! that resolves when the transport's internal delivery task drives that write
//! to a terminal [`SingleDeliveryOutcome`]. The transport owns that task, its
//! `spawn_blocking`, and any transport-local batching; the relay worker
//! concurrently submits new writes and collects resolved futures without
//! blocking on any single one.
//!
//! This retires the earlier "the sync core never crosses `.await`; the worker
//! owns `spawn_blocking`" invariant: ownership of the blocking delivery moves
//! into each transport. The legacy synchronous `deliver`/`prepare_delivery`/
//! `raw_write` seam has been removed now that every relay callsite delivers
//! through the write methods.
//!
//! ## Transport <-> relay interactions
//!
//! There is no generic inbound event channel. Each transport->relay interaction
//! uses its natural primitive:
//!
//! - **Choices** (tool-call permissions, and any future operator decision) are
//!   blocking requests: the relay injects a re-entrant [`Chooser`] via
//!   [`StartupContext`], which the transport invokes inline and blocks on until
//!   the operator decides. No transport->relay back-edge: the transport holds an
//!   opaque `Arc<dyn Fn>` typed here in `transports`.
//! - **Completion** resolves through the [`OutcomeFuture`] returned by
//!   [`Transport::mailw`]/[`Transport::raww`]: the transport's internal delivery
//!   task drives each write to a terminal [`SingleDeliveryOutcome`], and the
//!   worker fans out from the resolved future.
//! - **Output for `look`** is a concurrent read via [`Transport::give_output`],
//!   which hands the relay an [`OutputView`] handle the look request path can
//!   read without borrowing the worker-owned transport.
//!
//! ## Status
//!
//! Complete (`decouple-transport-layer`): the ACP transport lives in
//! `crate::acp` (driven by the `AcpWorkerDriver` lifecycle behind
//! [`TransportImpl::Acp`]) and the tmux transport in `crate::tmux`. The relay
//! delivery worker holds a [`TransportImpl`] per target and dispatches every
//! agent delivery through it.
//!
//! ## Layout
//!
//! This module owns the shared delivery types every signature below names —
//! [`DeliveryEnvelope`], [`DeliveryMessage`], [`SingleDeliveryOutcome`], the
//! choice types reached through [`Chooser`], [`StartupContext`], and the status
//! and look-window types. The traits live in the private `transport` child and
//! the dispatch enum in the private `dispatch` child, both re-exported here.
//!
//! The children are private deliberately: they are an internal division of one
//! contract, not a second addressable surface. Every path that resolved at
//! `transports::contract::*` before the split still resolves through these
//! re-exports, and callers reaching these through `crate::transports` are
//! unaffected either way.

mod dispatch;
mod transport;

pub use dispatch::{HandoverDimensions, TransportImpl};
pub use transport::{
    GenerationFence, OutputView, PartitionSink, Transport, TransportHealth, UnreachableSince,
};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;

use crate::configuration::BundleMember;
// Re-export the configuration prompt-readiness template into the transport
// contract namespace; the Tmux prompt probe consumes it and the transport wires it through
// the delivery context. It is defined once in `configuration`; re-exporting
// (rather than redefining) keeps the two in lockstep.
pub use crate::configuration::PromptReadinessTemplate;
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
/// ([`Transport::mailw`], [`Transport::raww`]). It resolves to the terminal
/// [`SingleDeliveryOutcome`] once the transport's internal delivery task settles
/// the write; the sender half lives inside the transport's ordered channel item.
///
/// Carries the transport-side [`SingleDeliveryOutcome`], not the relay
/// `SendResult`: the transport contract never depends on `crate::relay`, so the
/// relay worker maps the resolved outcome onto its own `SendResult` at the
/// collect site.
pub type OutcomeFuture = oneshot::Receiver<SingleDeliveryOutcome>;

/// Relay-provided, synchronous resolver for operator choices (tool-call
/// permissions today; any operator decision later).
///
/// Injected once at [`startup`](Transport::startup), so the transport depends
/// only downward on `transports`, never on `crate::relay`: the transport holds
/// an opaque `Arc<dyn Fn>`; the relay constructs it closing over its choice
/// queue. The transport invokes it on its own thread and BLOCKS until the
/// operator decides, preserving "the agent turn does not progress past a pending
/// choice."
///
/// RE-ENTRANT: the relay implementation keys per-request state by a generated
/// choice id and guards the shared queue with a mutex plus a per-request
/// condvar, so concurrent invocations (multiple permission requests in one turn)
/// each manage a distinct choice safely. INVARIANT: it MUST unblock and return
/// [`ChoiceMade::Cancelled`] on relay shutdown or respawn invalidation.
pub type Chooser = Arc<dyn Fn(ChoiceToMake) -> ChoiceMade + Send + Sync>;

/// A pending choice handed to the [`Chooser`]. The per-delivery correlation
/// fields (`message_id`, `target_session`, `decider_sessions`) are sourced from
/// the [`DeliveryEnvelope`] the transport's internal delivery task is submitting
/// when it raises a choice, since the startup-time chooser cannot close over
/// them. The queue bound (`choices_pending_max`) is a per-bundle constant the
/// chooser captures at construction, so it is not carried here.
#[derive(Clone, Debug)]
pub struct ChoiceToMake {
    /// Transport-native request id used to correlate the operator's response.
    pub request_id: u64,
    /// The originating send's message id (choice event correlation).
    pub message_id: String,
    /// The target session the choice belongs to.
    pub target_session: String,
    /// Sessions authorized to decide this choice.
    pub decider_sessions: Vec<String>,
    /// Human-facing title for the choice (for example, a tool-call title).
    pub title: String,
    /// The category of choice (for example, the requested permission kind).
    pub species: String,
    /// Transport-native detail payload for the choice.
    pub details: Value,
    /// The options the operator may choose among.
    pub options: Vec<ThingToChoose>,
}

/// One selectable option within a [`ChoiceToMake`].
#[derive(Clone, Debug)]
pub struct ThingToChoose {
    pub option_id: String,
    pub name: String,
    pub species: String,
}

/// The resolution of a [`ChoiceToMake`], returned by the [`Chooser`]. Mirrors
/// the relay's choice-resolution taxonomy so the transport's internal delivery
/// task can build the same terminal outcome.
#[derive(Clone, Debug)]
pub enum ChoiceMade {
    /// An option was chosen; carries the option id and who decided.
    Chosen {
        option_id: String,
        decided_by: String,
    },
    /// The choice was cancelled; carries the cancellation taxonomy (queue full,
    /// queue unavailable, user cancelled, shutdown, respawn invalidation).
    Cancelled {
        decided_by: String,
        reason_code: String,
        reason: Option<String>,
    },
}

/// Inputs required to establish a transport runtime for one target.
#[derive(Clone)]
pub struct StartupContext {
    pub namespace: String,
    pub runtime_directory: PathBuf,
    pub target_member: BundleMember,
    /// Relay-injected, re-entrant resolver for operator choices. See [`Chooser`].
    pub choose: Chooser,
}

impl std::fmt::Debug for StartupContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartupContext")
            .field("namespace", &self.namespace)
            .field("runtime_directory", &self.runtime_directory)
            .field("target_member", &self.target_member)
            .field("choose", &"<Chooser>")
            .finish()
    }
}

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
    /// delivery, threaded to [`ChoiceToMake::decider_sessions`].
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

/// The result of a [`Transport::startup`] call.
#[derive(Clone, Debug)]
pub struct TransportStatus {
    pub readiness: TransportReadiness,
}

/// Readiness of a transport runtime after startup.
#[derive(Clone, Debug)]
pub enum TransportReadiness {
    /// Ready to accept delivery immediately.
    Ready,
    /// Established but not yet ready (for example, awaiting first prompt).
    Pending,
    /// Could not be established; carries the failure taxonomy.
    Unavailable { code: String, reason: String },
}

/// A structured transport failure surfaced to the relay worker.
#[derive(Clone, Debug)]
pub struct TransportError {
    pub code: String,
    pub reason: String,
    pub details: Option<Value>,
}

/// Windowing parameters for an [`OutputView::look`] snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct LookMode {
    /// Window size (tmux pane lines or ACP replay entries).
    pub lines: Option<u64>,
    /// Entries to skip from the newest end before the tail window (ACP only).
    pub offset: Option<u64>,
    /// How long the handle may wait for a still-initializing target to populate
    /// its first snapshot before returning a stale-tagged result. The relay
    /// supplies this as its look-surface policy; a zero duration means no wait.
    pub prime_timeout: Duration,
}
