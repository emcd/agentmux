//! First-class UI transport.
//!
//! Delivers a relay-framed message to a target's registered UI subscribers as a
//! relay stream broadcast. Promoting UI to a transport retires the relay-internal
//! `Acp/Tmux/Ui/Pubsub` routing fork: the delivery worker now submits `mailw`
//! against a [`TransportImpl::Ui`](crate::transports::TransportImpl) like any
//! other target, and this module owns the broadcast.
//!
//! ## Relay touchpoints as injected closures
//!
//! The broadcast reaches the relay's stream registry (`send_event_to_registered_ui`)
//! and emits relay stream events. Both are injected as opaque `Arc<dyn Fn>`s in
//! [`UiTransportServices`], constructed relay-side closing over the target's
//! `(namespace, target_session)`; the transport invokes them without a
//! back-edge, so `src/transports` never imports `crate::relay` (mirrors the
//! `AcpDriverServices` pattern).
//!
//! ## UI delivery == broadcast accepted
//!
//! The TUI is a passive subscriber: there is no per-recipient render ack. A UI
//! delivery succeeds when the stream event reaches a registered, connected UI
//! endpoint. If none is connected the delivery resolves `not_submitted` at once,
//! from that one attempt — nothing was emitted to any subscriber, which the
//! transport observed rather than inferred, and a message sent to an unwatched
//! target is not held waiting for someone to start watching.
//! The transport builds its `incoming_message` event directly from
//! the envelope's structured [`DeliveryMessage`](crate::transports::DeliveryMessage):
//! it reads the relay-authored attribution as-is and never parses pane-envelope
//! text.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use tokio::sync::oneshot;

use crate::runtime::signals::shutdown_requested;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{
    DeliveryEnvelope, GenerationFence, OutputView, SendOutcome, SingleDeliveryOutcome,
    StartupContext, Transport, TransportError, TransportHealth, TransportReadiness,
    TransportStatus, stopped_before_submission_outcome,
};

const UI_RAW_WRITE_UNSUPPORTED_CODE: &str = "ui_raw_write_unsupported";
const UI_GENERATION_FENCED_CODE: &str = "ui_generation_fenced";
const UI_NO_ENDPOINT_CODE: &str = "ui_no_endpoint";

/// Transport-side outcome of one UI stream-broadcast attempt. Keeps the relay's
/// stream-send taxonomy out of `transports`: the injected closures map the
/// relay `StreamEventSendOutcome` onto this before handing it back.
#[derive(Clone, Debug)]
pub enum UiBroadcastStatus {
    /// The event reached a registered, connected UI endpoint.
    Delivered,
    /// No UI endpoint is registered, or the registered one is disconnected.
    /// Positive evidence that nothing reached a subscriber, which is why the
    /// delivery may resolve `not_submitted` on it rather than an unknown.
    NoUi,
    /// The broadcast failed irrecoverably; carries the relay-side reason.
    Failed(String),
}

/// The per-message payload the relay closure renders into the `incoming_message`
/// stream event. All ids arrive canonical (relay-populated); the transport never
/// derives attribution itself.
#[derive(Clone, Debug)]
pub struct UiIncomingMessage {
    pub message_id: String,
    pub sender_session: String,
    pub body: String,
    pub cc_sessions: Vec<String>,
    pub authenticated_identity: Option<String>,
    pub on_behalf_of: Option<String>,
}

/// One `delivery_outcome` phase the transport asks the relay to emit
/// (`routed` / `delivered` / `failed`).
#[derive(Clone, Debug)]
pub struct UiOutcomePhase {
    pub message_id: String,
    pub phase: &'static str,
    pub outcome: Option<&'static str>,
    pub reason_code: Option<String>,
    pub reason: Option<String>,
}

/// Emits the `incoming_message` stream event for a delivery; returns whether it
/// reached a connected UI.
pub type UiBroadcastFn = Arc<dyn Fn(&UiIncomingMessage) -> UiBroadcastStatus + Send + Sync>;
/// Emits a `delivery_outcome` phase event; returns whether it reached a
/// connected UI.
pub type UiPhaseFn = Arc<dyn Fn(UiOutcomePhase) -> UiBroadcastStatus + Send + Sync>;

/// Relay-provided UI broadcast touchpoints, injected once when the transport is
/// built. Each closure closes over the target's `(namespace, target_session)`
/// and the relay stream registry; the transport holds opaque `Arc<dyn Fn>`s, so
/// `src/transports` never imports `crate::relay`.
#[derive(Clone)]
pub struct UiTransportServices {
    pub broadcast_incoming: UiBroadcastFn,
    pub emit_phase: UiPhaseFn,
}

impl std::fmt::Debug for UiTransportServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiTransportServices")
            .finish_non_exhaustive()
    }
}

/// Delivers relay messages to a target's registered UI subscribers.
///
/// Held by `TransportImpl::Ui`. [`mailw`](Transport::mailw) broadcasts the
/// message as a stream event on its own thread (so the write stays non-blocking)
/// and resolves the [`OutcomeFuture`] from what that one attempt proved. The UI
/// is not raw-writable ([`raww`](Transport::raww) resolves an unsupported
/// outcome) and not lookable ([`give_output`](Transport::give_output) is `None`).
///
/// **There is no reconnect wait.** A bounded poll for a UI to come back was an
/// absence timer with a budget only this transport knew, and elapsed time was
/// deciding an outcome. The delivery now attempts once and reports what that
/// attempt proved: an absent subscriber is a fact at the moment of the
/// broadcast, not something to wait out.
#[derive(Debug)]
pub struct UiTransport {
    services: UiTransportServices,
    /// Shared with every delivery executor this generation spawns, so the
    /// generation can be asked to stop and then stripped of its ability to emit.
    generation: Arc<UiGeneration>,
    /// Handles for the delivery executors this generation owns. Retained rather
    /// than detached: an executor whose handle is discarded cannot be observed,
    /// and therefore cannot be fenced.
    executors: Vec<thread::JoinHandle<()>>,
}

/// The fencing state one UI generation shares with its delivery executors.
///
/// Two flags rather than one, because the fence's steps 1 and 3 are genuinely
/// different actions. `fenced` asks an executor to stop at its next check and
/// costs nothing when it works. `revoked` is the forced step: it strips the
/// generation of its ability to emit, so an executor that proceeds anyway
/// produces no further frame.
#[derive(Debug, Default)]
struct UiGeneration {
    fenced: AtomicBool,
    revoked: AtomicBool,
}

impl UiGeneration {
    fn is_fenced(&self) -> bool {
        self.fenced.load(Ordering::Acquire)
    }

    /// Whether this generation has been stripped of its ability to emit.
    ///
    /// Checked immediately before the broadcast that actually delivers the
    /// message, which is the one emit that must not happen after forced
    /// termination. It is the guard that matters for an executor blocked inside
    /// a broadcast when the fence began: it will not have seen the cooperative
    /// flag, but it cannot deliver once revoked.
    fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }
}

impl UiTransport {
    #[must_use]
    pub fn new(services: UiTransportServices) -> Self {
        Self {
            services,
            generation: Arc::new(UiGeneration::default()),
            executors: Vec::new(),
        }
    }
}

impl GenerationFence for UiTransport {
    fn fence_generation(&mut self) {
        self.generation.fenced.store(true, Ordering::Release);
    }

    fn terminate_generation(&mut self) {
        // UI owns no child and holds no process to signal. Revoking the
        // generation's ability to emit is the equivalent action: it drops the
        // broadcaster's effect for every executor still running, which is what
        // unblocks the observation below from succeeding.
        self.generation.revoked.store(true, Ordering::Release);
    }

    fn generation_ceased(&self) -> bool {
        self.executors.iter().all(thread::JoinHandle::is_finished)
    }
}

impl Transport for UiTransport {
    fn startup(&mut self, _context: StartupContext) -> Result<TransportStatus, TransportError> {
        // The UI transport has no runtime to establish; it is always ready and
        // resolves connectivity per-delivery, from the broadcast itself.
        Ok(TransportStatus {
            readiness: TransportReadiness::Ready,
        })
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let (sender, receiver) = oneshot::channel();
        let services = self.services.clone();
        let message_id = envelope.message_id.clone();
        let message = envelope.message;
        let incoming = UiIncomingMessage {
            message_id: envelope.message_id,
            // Machine-consumed event fields carry the bare canonical id via the
            // non-decorating accessor — never render_address (which decorates to
            // "Display Name <session:session_name>" for the pane header).
            sender_session: message.sender.canonical_session_id().to_string(),
            body: message.body,
            cc_sessions: message
                .cc
                .iter()
                .map(|party| party.canonical_session_id().to_string())
                .collect(),
            authenticated_identity: message.authenticated_identity,
            on_behalf_of: message.on_behalf_of,
        };
        // Still on its own thread even though the delivery no longer waits: the
        // broadcast writes to a socket, and `mailw` is contractually
        // non-blocking at the relay boundary.
        let generation = Arc::clone(&self.generation);
        let handle = thread::spawn(move || {
            let outcome = run_ui_delivery(&services, message_id, &incoming, generation.as_ref());
            let _ = sender.send(outcome);
        });
        // Drop the handles of executors that have already finished, so a
        // long-lived transport does not accumulate one per delivery it ever
        // made. Only live executors need observing.
        self.executors.retain(|handle| !handle.is_finished());
        self.executors.push(handle);
        receiver
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let _ = (content, append_enter);
        let (sender, receiver) = oneshot::channel();
        let _ = sender.send(SingleDeliveryOutcome {
            target_session: String::new(),
            message_id: String::new(),
            outcome: SendOutcome::Failed,
            reason_code: Some(UI_RAW_WRITE_UNSUPPORTED_CODE.to_string()),
            reason: Some("UI transport does not support raw input".to_string()),
            details: None,
        });
        receiver
    }

    async fn is_ready_for_handover(&self) -> bool {
        true
    }

    fn health(&self) -> TransportHealth {
        // A UI target is a broadcast surface, not a process the transport can
        // lose track of: with no subscribers the broadcast is delivered to
        // nobody, which is an empty audience rather than an unreachable target.
        // Nothing here can fail to be observed, so nothing can start a dwell.
        //
        // Reporting unreachability instead was tried and withdrawn. It would let
        // a member wait out `[delivery].unreachable-dwell-ms` for a UI to come
        // back, which sounds like the reconnect wait's useful half — but nothing
        // replays to a UI that reconnects, so the wait only helps a message that
        // is still queued when the endpoint returns, and it costs a queued
        // member its immediate honest answer. An absent subscriber is reported
        // at the broadcast instead, where it is a fact rather than a forecast.
        TransportHealth::Healthy
    }

    fn shutdown(&mut self) {}

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        None
    }
}

/// Drives one UI delivery to a terminal [`SingleDeliveryOutcome`] in a single
/// pass, with no wait for a UI to (re)connect. Ports the relay's former
/// `deliver_one_target_ui` loop onto the injected broadcast closures: the
/// `routed` phase announces the attempt, the `incoming_message` broadcast
/// delivers the body and is where an absent endpoint is discovered, and
/// `delivered`/`failed` phases mirror the terminal state to the UI.
fn run_ui_delivery(
    services: &UiTransportServices,
    message_id: String,
    incoming: &UiIncomingMessage,
    generation: &UiGeneration,
) -> SingleDeliveryOutcome {
    // The cooperative stop check. Resolving `not_submitted` is sound rather than
    // conservative: no broadcast has been attempted, so nothing can have reached
    // a subscriber.
    if generation.is_fenced() {
        return terminal(
            message_id,
            SendOutcome::NotSubmitted,
            Some(UI_GENERATION_FENCED_CODE.to_string()),
            Some("UI generation was fenced before this delivery emitted".to_string()),
        );
    }
    if shutdown_requested() {
        // `not_submitted`, not `dropped_on_shutdown`. This member was authorized
        // and handed to a transport, and the contract reserves the shutdown
        // spelling for members the relay still holds as `Pending` — shutdown is
        // a trigger, and the evidence order chooses the outcome. The cause
        // survives in the reason code, which is the same split the coder
        // transports use.
        let stopped = stopped_before_submission_outcome(String::new(), message_id.clone());
        let _ = (services.emit_phase)(UiOutcomePhase {
            message_id,
            phase: "failed",
            outcome: Some("not_submitted"),
            reason_code: stopped.reason_code.clone(),
            reason: stopped.reason.clone(),
        });
        return stopped;
    }

    // `routed` announces the attempt; it no longer doubles as a reconnect probe,
    // because there is no reconnect wait for it to drive. A disconnected
    // endpoint is not anticipated on the health axis either — UI reports healthy
    // unconditionally — it is discovered at the broadcast below and reported
    // there as `ui_no_endpoint`.
    let routed = (services.emit_phase)(UiOutcomePhase {
        message_id: message_id.clone(),
        phase: "routed",
        outcome: None,
        reason_code: None,
        reason: None,
    });
    if let UiBroadcastStatus::Failed(reason) = routed {
        return terminal(message_id, SendOutcome::Failed, None, Some(reason));
    }

    if generation.is_revoked() {
        return terminal(
            message_id,
            SendOutcome::NotSubmitted,
            Some(UI_GENERATION_FENCED_CODE.to_string()),
            Some("UI generation was terminated before this delivery emitted".to_string()),
        );
    }
    match (services.broadcast_incoming)(incoming) {
        UiBroadcastStatus::Delivered => {
            let _ = (services.emit_phase)(UiOutcomePhase {
                message_id: message_id.clone(),
                phase: "delivered",
                outcome: Some("success"),
                reason_code: None,
                reason: None,
            });
            terminal(message_id, SendOutcome::Delivered, None, None)
        }
        // The broadcast found no endpoint, which may equally be because none was
        // ever registered — nothing upstream anticipates this, since UI reports
        // healthy unconditionally. Nothing was emitted to any subscriber, and
        // that is observed here rather than inferred from elapsed time, so the
        // transport is entitled to the strong spelling and resolves at once.
        UiBroadcastStatus::NoUi => terminal(
            message_id,
            SendOutcome::NotSubmitted,
            Some(UI_NO_ENDPOINT_CODE.to_string()),
            Some("no UI endpoint was registered for this target".to_string()),
        ),
        UiBroadcastStatus::Failed(reason) => {
            terminal(message_id, SendOutcome::Failed, None, Some(reason))
        }
    }
}

/// Builds a terminal outcome. `target_session` is left empty: the relay worker
/// substitutes each task's own `target_session`/`message_id` when it maps the
/// outcome onto its `SendResult` (mirroring the ACP/tmux fan-out).
fn terminal(
    message_id: String,
    outcome: SendOutcome,
    reason_code: Option<String>,
    reason: Option<String>,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session: String::new(),
        message_id,
        outcome,
        reason_code,
        reason,
        details: None,
    }
}
