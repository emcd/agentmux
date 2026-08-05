//! First-class UI transport.
//!
//! Delivers a relay-framed message to a target's registered UI subscribers as a
//! relay stream broadcast. Promoting UI to a transport retires the relay-internal
//! `Acp/Tmux/Ui/Pubsub` routing fork: the delivery worker now submits `mailw`
//! against a [`TransportImpl::Ui`](crate::transports::TransportImpl) like any
//! other target, and this module owns the broadcast + bounded UI-reconnect wait.
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
//! endpoint. If no UI is connected, the transport polls up to its internal
//! [`UI_RECONNECT_TIMEOUT_MS_DEFAULT`] cap for one to reconnect, then resolves
//! `Timeout`. The transport builds its `incoming_message` event directly from
//! the envelope's structured [`DeliveryMessage`](crate::transports::DeliveryMessage):
//! it reads the relay-authored attribution as-is and never parses pane-envelope
//! text.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::runtime::signals::shutdown_requested;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{
    DeliveryEnvelope, GenerationFence, OutputView, SendOutcome, SingleDeliveryOutcome,
    StartupContext, Transport, TransportError, TransportReadiness, TransportStatus,
};

const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const UI_RAW_WRITE_UNSUPPORTED_CODE: &str = "ui_raw_write_unsupported";
const UI_GENERATION_FENCED_CODE: &str = "ui_generation_fenced";
const UI_RECONNECT_ELAPSED_CODE: &str = "ui_reconnect_elapsed";
const UI_RECONNECT_POLL_INTERVAL_MS: u64 = 100;
/// Default cap on the UI reconnect wait when no UI endpoint is registered or the
/// registered one is disconnected. Owned entirely by the UI transport — the
/// relay no longer threads an external knob through the envelope for it
/// (`DeliveryEnvelope.quiescence_timeout` was removed in `tmux-wedge-detection`).
/// [`UiTransport::new`] seeds this; [`UiTransport::with_reconnect_timeout`]
/// overrides it (tests inject a short budget so the no-UI timeout path resolves
/// in milliseconds instead of blocking the full production window).
const UI_RECONNECT_TIMEOUT_MS_DEFAULT: u64 = 30_000;

/// Transport-side outcome of one UI stream-broadcast attempt. Keeps the relay's
/// stream-send taxonomy out of `transports`: the injected closures map the
/// relay `StreamEventSendOutcome` onto this before handing it back.
#[derive(Clone, Debug)]
pub enum UiBroadcastStatus {
    /// The event reached a registered, connected UI endpoint.
    Delivered,
    /// No UI endpoint is registered, or the registered one is disconnected; the
    /// caller may retry within its reconnect budget.
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
/// (`routed` / `delivered` / `failed`). The `routed` phase doubles as the
/// reconnect probe: its [`UiBroadcastStatus`] tells the transport whether a UI
/// is currently connected.
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
/// connected UI (used by the `routed` reconnect probe).
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
/// message as a stream event on its own thread (so the write stays non-blocking),
/// runs the bounded UI-reconnect wait, and resolves the [`OutcomeFuture`]. The UI
/// is not raw-writable ([`raww`](Transport::raww) resolves an unsupported
/// outcome) and not lookable ([`give_output`](Transport::give_output) is `None`).
#[derive(Debug)]
pub struct UiTransport {
    services: UiTransportServices,
    reconnect_timeout: Duration,
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
            reconnect_timeout: Duration::from_millis(UI_RECONNECT_TIMEOUT_MS_DEFAULT),
            generation: Arc::new(UiGeneration::default()),
            executors: Vec::new(),
        }
    }

    /// Overrides the UI-reconnect wait cap seeded by [`new`](Self::new). The
    /// production default is [`UI_RECONNECT_TIMEOUT_MS_DEFAULT`]; tests inject a
    /// short budget so the no-UI timeout path resolves in milliseconds instead
    /// of blocking the full production window.
    #[must_use]
    pub fn with_reconnect_timeout(mut self, reconnect_timeout: Duration) -> Self {
        self.reconnect_timeout = reconnect_timeout;
        self
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
        // resolves connectivity per-delivery via the reconnect wait.
        Ok(TransportStatus {
            readiness: TransportReadiness::Ready,
        })
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let (sender, receiver) = oneshot::channel();
        let services = self.services.clone();
        let timeout = self.reconnect_timeout;
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
        // Run the bounded reconnect wait off the async worker thread so `mailw`
        // stays non-blocking; resolve the future when it settles.
        let generation = Arc::clone(&self.generation);
        let handle = thread::spawn(move || {
            let outcome = run_ui_delivery(
                &services,
                timeout,
                message_id,
                &incoming,
                generation.as_ref(),
            );
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

    fn is_ready(&self) -> bool {
        true
    }

    fn can_accept_handover(&self) -> bool {
        true
    }

    fn shutdown(&mut self) {}

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        None
    }
}

/// Drives one UI delivery to a terminal [`SingleDeliveryOutcome`], polling for a
/// UI to (re)connect up to `timeout`. Ports the relay's former
/// `deliver_one_target_ui` loop onto the injected broadcast closures: the
/// `routed` phase probes connectivity, the `incoming_message` broadcast delivers
/// the body, and `delivered`/`failed` phases mirror the terminal state to the UI.
fn run_ui_delivery(
    services: &UiTransportServices,
    timeout: Duration,
    message_id: String,
    incoming: &UiIncomingMessage,
    generation: &UiGeneration,
) -> SingleDeliveryOutcome {
    let start = Instant::now();
    loop {
        // The cooperative stop check. Reached at the top of every reconnect
        // poll, so a fenced generation ceases within one poll interval without
        // needing the destructive step. Resolving `not_submitted` is sound
        // rather than conservative: no broadcast has been attempted on this
        // iteration, so nothing can have reached a subscriber.
        if generation.is_fenced() {
            return terminal(
                message_id,
                SendOutcome::NotSubmitted,
                Some(UI_GENERATION_FENCED_CODE.to_string()),
                Some("UI generation was fenced before this delivery emitted".to_string()),
            );
        }
        if shutdown_requested() {
            let _ = (services.emit_phase)(UiOutcomePhase {
                message_id: message_id.clone(),
                phase: "failed",
                outcome: Some("failed"),
                reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
            });
            return terminal(
                message_id,
                SendOutcome::DroppedOnShutdown,
                Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
            );
        }

        // `routed` phase doubles as the reconnect probe.
        match (services.emit_phase)(UiOutcomePhase {
            message_id: message_id.clone(),
            phase: "routed",
            outcome: None,
            reason_code: None,
            reason: None,
        }) {
            UiBroadcastStatus::Delivered => {}
            UiBroadcastStatus::NoUi => {
                if let Some(outcome) = timeout_if_elapsed(&message_id, start, timeout) {
                    return outcome;
                }
                thread::sleep(Duration::from_millis(UI_RECONNECT_POLL_INTERVAL_MS));
                continue;
            }
            UiBroadcastStatus::Failed(reason) => {
                return terminal(message_id, SendOutcome::Failed, None, Some(reason));
            }
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
                return terminal(message_id, SendOutcome::Delivered, None, None);
            }
            UiBroadcastStatus::NoUi => {}
            UiBroadcastStatus::Failed(reason) => {
                return terminal(message_id, SendOutcome::Failed, None, Some(reason));
            }
        }

        if let Some(outcome) = timeout_if_elapsed(&message_id, start, timeout) {
            return outcome;
        }
        thread::sleep(Duration::from_millis(UI_RECONNECT_POLL_INTERVAL_MS));
    }
}

/// Returns a `Timeout` outcome when the reconnect budget is exhausted, else
/// `None` so the caller polls again.
fn timeout_if_elapsed(
    message_id: &str,
    start: Instant,
    timeout: Duration,
) -> Option<SingleDeliveryOutcome> {
    if start.elapsed() >= timeout {
        // `not_submitted`, not `timeout`. The reconnect wait elapses only on the
        // `NoUi` branch, which means no broadcast was ever attempted — so this is
        // positive evidence that nothing reached a subscriber, and the transport
        // is entitled to say so. Reporting elapsed time as its own outcome would
        // be the absence-inferred spelling this delivery contract retires, and it
        // would collapse into `submission_unknown` at the guard, understating
        // what the transport actually knows.
        return Some(terminal(
            message_id.to_string(),
            SendOutcome::NotSubmitted,
            Some(UI_RECONNECT_ELAPSED_CODE.to_string()),
            Some(format!(
                "ui relay stream was disconnected for {}ms",
                start.elapsed().as_millis()
            )),
        ));
    }
    None
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
