//! The envelope a transport is handed and the outcome it reports back.
//!
//! [`SingleDeliveryOutcome`] is the terminal result an [`OutcomeFuture`]
//! resolves to. Nothing here imports `crate::relay` — the relay maps the
//! resolved outcome onto its own `SendResult` at the collect site.
//!
//! [`DeliveryEnvelope`] and [`DeliveryMessage`] are re-exported rather than
//! defined here. Both call directions name them, so they live in
//! [`crate::protocol`] and neither side owns them.

use serde_json::Value;
use tokio::sync::oneshot;

use crate::runtime::signals::shutdown_requested;
// Delivery/look wire vocabulary. Canonical home is `crate::protocol` (below both
// the relay and the transports in dependency order), so the transport contract
// never depends on relay. The relay re-exports these from its own contract.
use crate::protocol::SendOutcome;
pub use crate::protocol::{DeliveryEnvelope, DeliveryMessage};

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
