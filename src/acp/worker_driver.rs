//! The ACP worker lifecycle driver.
//!
//! [`AcpWorkerDriver`] owns the per-target [`AcpTransport`] and its
//! bootstrap/respawn lifecycle. It is held by `TransportImpl::Acp`, so the relay
//! delivery worker drives ACP startup and recovery through the generic transport
//! handle without naming any ACP type. The driver depends only downward on
//! `crate::transports`, `crate::configuration`, and `crate::runtime` — never on
//! `crate::relay`.
//!
//! ## Relay touchpoints as injected closures
//!
//! The lifecycle reaches relay-owned registries (worker-state mirror, look
//! OutputView publish, choice-queue invalidation, UI stream broadcast) and the
//! relay choice queue (the [`Chooser`]). Each is injected as an opaque
//! `Arc<dyn Fn>` (or value) in [`AcpDriverServices`], constructed relay-side
//! closing over relay services; the driver invokes them without a back-edge,
//! mirroring the `Chooser` pattern from Slice 2b.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::configuration::BundleMember;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::signals::shutdown_requested;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{
    AcpWorkerReadinessState, Chooser, DeliveryContext, DeliveryEnvelope, DeliveryPreparation,
    DeliveryResult, DeliveryWaitError, OutputView, RawWriteResult, StartupContext, Transport,
    TransportError, TransportStatus,
};

use super::{
    ACP_ERROR_CODE_CONNECTION_CLOSED, ACP_ERROR_CODE_INITIALIZE_FAILED,
    ACP_ERROR_CODE_PROMPT_FAILED, ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE, AcpBootstrapError,
    AcpTransport,
};

const RESPAWN_BACKOFF_MAX_MS_ENVVAR: &str = "AGENTMUX_RELAY_ACP_RESPAWN_BACKOFF_MAX_MS";
const RESPAWN_SLEEP_POLL_MS: u64 = 50;
const RESPAWN_BACKOFF_INITIAL_MS: u64 = 1_000;
const RESPAWN_BACKOFF_CAP_DEFAULT_MS: u64 = 30_000;
const RESPAWN_INIT_FAILURE_THRESHOLD: u32 = 3;

/// Mirrors the worker readiness state into the relay's global registry.
pub type MirrorStateFn = Arc<dyn Fn(AcpWorkerReadinessState) + Send + Sync>;
/// Publishes the transport's `look` [`OutputView`] handle into the relay registry.
pub type PublishOutputFn = Arc<dyn Fn(Option<Arc<dyn OutputView>>) + Send + Sync>;
/// Broadcasts an ACP respawn stream event (`event_type`, `payload`) to the bundle UI.
pub type BroadcastUiFn = Arc<dyn Fn(&str, Value) + Send + Sync>;
/// Invalidates the target's pending operator choices before a respawn attempt.
pub type InvalidateChoicesFn = Arc<dyn Fn() + Send + Sync>;

/// Relay-provided lifecycle touchpoints, injected once when the driver is built.
///
/// Each closure closes over the relay's own registries/services for one target;
/// the driver holds opaque `Arc<dyn Fn>`s typed only in `transports`, so
/// `src/acp` never imports `crate::relay`.
#[derive(Clone)]
pub struct AcpDriverServices {
    /// Mirrors the worker readiness state into the relay's global registry (the
    /// TUI worker-state stream and the relay's own respawn gate observe it).
    pub mirror_state: MirrorStateFn,
    /// Publishes the transport's `look` [`OutputView`] handle into the relay
    /// look registry. Called before each `startup` so a `look` racing init finds
    /// the handle and runs its bounded prime-wait.
    pub publish_output: PublishOutputFn,
    /// Broadcasts an ACP respawn stream event (`event_type`, `payload`) to the
    /// bundle's registered UI sessions. The relay closure wraps it in its own
    /// `RelayStreamEvent`.
    pub broadcast_ui: BroadcastUiFn,
    /// Invalidates the target's pending operator choices before a respawn
    /// attempt, logging its own failure. Encapsulates the relay choice-queue
    /// context construction.
    pub invalidate_choices: InvalidateChoicesFn,
    /// Re-entrant operator-choice resolver threaded into every [`StartupContext`].
    pub chooser: Chooser,
}

impl std::fmt::Debug for AcpDriverServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpDriverServices").finish_non_exhaustive()
    }
}

/// Owns the per-target ACP transport and its bootstrap/respawn lifecycle.
///
/// Held by `TransportImpl::Acp`. Delivery trait methods delegate to the inner
/// [`AcpTransport`]; the async `bootstrap`/`respawn` lifecycle and the
/// readiness-mirroring helpers are inherent methods the relay worker drives via
/// the `TransportImpl::Acp` match.
pub struct AcpWorkerDriver {
    transport: Option<AcpTransport>,
    respawn_state: AcpRespawnState,
    bundle_name: String,
    runtime_directory: PathBuf,
    target_member: BundleMember,
    services: AcpDriverServices,
    max_prompt_tokens: usize,
}

impl AcpWorkerDriver {
    /// Builds a driver for one ACP target with a fresh transport.
    #[must_use]
    pub fn new(
        target_member: BundleMember,
        runtime_directory: PathBuf,
        bundle_name: String,
        services: AcpDriverServices,
        max_prompt_tokens: usize,
    ) -> Self {
        Self {
            transport: Some(AcpTransport::new(max_prompt_tokens)),
            respawn_state: AcpRespawnState::new(),
            bundle_name,
            runtime_directory,
            target_member,
            services,
            max_prompt_tokens,
        }
    }

    /// Per-prompt token budget captured at construction, threaded from session
    /// configuration. Handed to the internal ACP delivery task (write-interface
    /// refactor section 3) when combining a coalesced envelope group into one turn.
    #[must_use]
    pub fn max_prompt_tokens(&self) -> usize {
        self.max_prompt_tokens
    }

    /// Checks if the internal delivery task signaled that a respawn is needed.
    /// Returns the trigger reason if a respawn is needed, `None` otherwise.
    /// The relay worker should call `maybe_respawn_after_delivery` when this
    /// returns `Some`.
    pub fn check_respawn_needed(&mut self) -> Option<String> {
        self.transport
            .as_mut()
            .expect("acp driver transport present")
            .check_respawn_needed()
    }

    fn transport_ref(&self) -> &AcpTransport {
        self.transport
            .as_ref()
            .expect("acp driver transport present")
    }

    fn target_session(&self) -> &str {
        self.target_member.id.as_str()
    }

    fn startup_context(&self) -> StartupContext {
        StartupContext {
            bundle_name: self.bundle_name.clone(),
            runtime_directory: self.runtime_directory.clone(),
            target_member: self.target_member.clone(),
            choose: self.services.chooser.clone(),
        }
    }

    /// Establishes the ACP runtime when the worker starts. Mirrors the readiness
    /// transitions and publishes the look handle before `startup` runs, so a
    /// `look` in the initial-startup window finds the handle. The transport (and
    /// its published handle) is kept even on failure: the worker drives respawn
    /// off the Unavailable state and reuses this transport.
    pub async fn bootstrap(&mut self) {
        (self.services.mirror_state)(AcpWorkerReadinessState::Initializing);
        (self.services.publish_output)(self.transport_ref().give_output());
        let startup_context = self.startup_context();
        let transport = self
            .transport
            .take()
            .expect("acp driver transport present at bootstrap");
        let (transport, result) = tokio::task::spawn_blocking(move || {
            let mut transport = transport;
            let result = transport.startup(startup_context);
            (transport, result)
        })
        .await
        .expect("ACP worker bootstrap task panicked");
        self.transport = Some(transport);

        match &result {
            Ok(_) => (self.services.mirror_state)(AcpWorkerReadinessState::Available),
            Err(error) => {
                (self.services.mirror_state)(AcpWorkerReadinessState::Unavailable);
                emit_inscription(
                    "relay.acp.worker.bootstrap_failed",
                    &json!({
                        "bundle_name": self.bundle_name,
                        "target_session": self.target_session(),
                        "error_code": error.code,
                        "reason": error.reason,
                    }),
                );
            }
        }
    }

    /// Marks the target Busy before a delivery batch so external observers see
    /// the in-turn transition.
    pub fn mark_busy(&self) {
        (self.services.mirror_state)(AcpWorkerReadinessState::Busy);
    }

    /// Mirrors the transport's settled readiness after `deliver` returns
    /// (`deliver` folds in completion, so readiness is final on return).
    pub fn mirror_settled_readiness(&self) {
        (self.services.mirror_state)(self.transport_ref().readiness());
    }

    /// Drives respawn when the post-delivery readiness is Unavailable, otherwise
    /// resets the backoff. `reason_code` is the head outcome's reason code, used
    /// to classify the respawn trigger.
    pub async fn maybe_respawn_after_delivery(&mut self, reason_code: Option<String>) {
        match self.transport_ref().readiness() {
            AcpWorkerReadinessState::Unavailable => {
                let trigger_reason = classify_respawn_trigger(reason_code.as_deref());
                self.respawn(trigger_reason).await;
            }
            AcpWorkerReadinessState::Available | AcpWorkerReadinessState::Busy => {
                self.respawn_state.reset_on_success();
            }
            AcpWorkerReadinessState::Initializing | AcpWorkerReadinessState::Recovering => {}
        }
    }

    /// Releases the dead runtime and re-establishes it with capped exponential
    /// backoff, mirroring Recovering/Available/Unavailable transitions,
    /// broadcasting respawn stream events, and invalidating pending choices
    /// before each attempt. Returns when startup succeeds, the failure is
    /// permanent, the retry budget is exhausted, or shutdown is requested.
    pub async fn respawn(&mut self, trigger_reason: &'static str) {
        // Release the dead runtime (joining its child + reader thread) but keep
        // the transport and its published handle, marking it recovering. A look
        // racing the respawn reads a recovering/stale snapshot through the
        // still-valid handle rather than the dead buffer or a missing handle.
        if let Some(transport) = self.transport.as_mut() {
            transport.release_runtime();
        }

        loop {
            if shutdown_requested() {
                return;
            }
            let backoff = self.respawn_state.advance();
            (self.services.mirror_state)(AcpWorkerReadinessState::Recovering);
            emit_inscription(
                "relay.acp.respawn.triggered",
                &json!({
                    "bundle_name": self.bundle_name,
                    "target_session": self.target_session(),
                    "attempt": self.respawn_state.attempt,
                    "trigger_reason": trigger_reason,
                    "backoff_ms": backoff.as_millis() as u64,
                }),
            );
            (self.services.broadcast_ui)(
                "acp_worker_respawn_started",
                json!({
                    "attempt": self.respawn_state.attempt,
                    "trigger_reason": trigger_reason,
                    "backoff_ms": backoff.as_millis() as u64,
                }),
            );

            if !sleep_with_shutdown_gate(backoff).await {
                return;
            }

            (self.services.invalidate_choices)();

            // Reuse the existing transport so its published handle stays valid
            // across the respawn; create a fresh one (re-publishing the handle)
            // only if it is somehow absent.
            let publish_output = Arc::clone(&self.services.publish_output);
            let max_prompt_tokens = self.max_prompt_tokens;
            let transport = self.transport.take().unwrap_or_else(|| {
                let transport = AcpTransport::new(max_prompt_tokens);
                publish_output(transport.give_output());
                transport
            });
            let startup_context = self.startup_context();
            let (transport, respawn_result) = tokio::task::spawn_blocking(move || {
                let mut transport = transport;
                let result = transport.startup(startup_context);
                (transport, result)
            })
            .await
            .expect("ACP respawn task panicked");

            match respawn_result {
                Ok(_) => {
                    // The published handle is still valid; startup repointed its
                    // replay slot, so no re-install is needed.
                    (self.services.mirror_state)(AcpWorkerReadinessState::Available);
                    emit_inscription(
                        "relay.acp.respawn.succeeded",
                        &json!({
                            "bundle_name": self.bundle_name,
                            "target_session": self.target_session(),
                            "attempt": self.respawn_state.attempt,
                        }),
                    );
                    (self.services.broadcast_ui)(
                        "acp_worker_respawn_completed",
                        json!({
                            "attempt": self.respawn_state.attempt,
                            "outcome": "succeeded",
                        }),
                    );
                    self.transport = Some(transport);
                    self.respawn_state.reset_on_success();
                    return;
                }
                Err(error) => {
                    // Put the transport back so the next attempt reuses it (the
                    // handle stays valid throughout).
                    self.transport = Some(transport);
                    // `startup` reports `TransportError`; the respawn classifier
                    // and permanence check still speak `AcpBootstrapError` (same
                    // code).
                    let error = AcpBootstrapError {
                        code: error.code,
                        reason: error.reason,
                    };
                    self.respawn_state.record_failure(&error);
                    emit_inscription(
                        "relay.acp.respawn.attempt_failed",
                        &json!({
                            "bundle_name": self.bundle_name,
                            "target_session": self.target_session(),
                            "attempt": self.respawn_state.attempt,
                            "error_code": error.code,
                            "reason": error.reason,
                        }),
                    );
                    if error.is_permanent() || self.respawn_state.should_give_up() {
                        (self.services.mirror_state)(AcpWorkerReadinessState::Unavailable);
                        emit_inscription(
                            "relay.acp.respawn.permanent_failure",
                            &json!({
                                "bundle_name": self.bundle_name,
                                "target_session": self.target_session(),
                                "attempts": self.respawn_state.attempt,
                                "final_error_code": error.code,
                                "reason": error.reason,
                            }),
                        );
                        (self.services.broadcast_ui)(
                            "acp_worker_respawn_completed",
                            json!({
                                "attempts": self.respawn_state.attempt,
                                "outcome": "permanent_failure",
                                "final_error_code": error.code,
                                "reason": error.reason,
                            }),
                        );
                        return;
                    }
                }
            }
        }
    }
}

impl Transport for AcpWorkerDriver {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        self.transport
            .as_mut()
            .expect("acp driver transport present")
            .startup(context)
    }

    fn prepare_delivery(
        &self,
        context: &DeliveryContext,
    ) -> Result<DeliveryPreparation, DeliveryWaitError> {
        self.transport_ref().prepare_delivery(context)
    }

    fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult {
        self.transport
            .as_mut()
            .expect("acp driver transport present")
            .deliver(envelopes, context)
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        self.transport
            .as_mut()
            .expect("acp driver transport present")
            .mailw(envelope)
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        self.transport
            .as_mut()
            .expect("acp driver transport present")
            .raww(content, append_enter)
    }

    fn is_ready(&self) -> bool {
        self.transport_ref().is_ready()
    }

    fn raw_write(
        &mut self,
        text: &str,
        append_enter: bool,
        context: &DeliveryContext,
    ) -> RawWriteResult {
        self.transport
            .as_mut()
            .expect("acp driver transport present")
            .raw_write(text, append_enter, context)
    }

    fn shutdown(&mut self) {
        if let Some(transport) = self.transport.as_mut() {
            transport.shutdown();
        }
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        self.transport_ref().give_output()
    }
}

/// Maps the head outcome's reason code to the respawn trigger label. A missing
/// code (an `Err` outcome or an `Ok` without a code) is a generic worker
/// unavailability.
fn classify_respawn_trigger(reason_code: Option<&str>) -> &'static str {
    match reason_code {
        Some(code) if code == ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE => "transport_unavailable",
        Some(code) if code == ACP_ERROR_CODE_PROMPT_FAILED => "serialization_failed",
        Some(code) if code == ACP_ERROR_CODE_CONNECTION_CLOSED => "connection_closed",
        _ => "worker_unavailable",
    }
}

async fn sleep_with_shutdown_gate(duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if shutdown_requested() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let poll = remaining.min(Duration::from_millis(RESPAWN_SLEEP_POLL_MS));
        if poll.is_zero() {
            break;
        }
        tokio::time::sleep(poll).await;
    }
    !shutdown_requested()
}

fn respawn_backoff_cap_ms() -> u64 {
    std::env::var(RESPAWN_BACKOFF_MAX_MS_ENVVAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(RESPAWN_BACKOFF_CAP_DEFAULT_MS)
}

struct AcpRespawnState {
    attempt: u32,
    next_backoff_ms: u64,
    last_initialize_failure_reason: Option<String>,
    consecutive_initialize_failures: u32,
}

impl AcpRespawnState {
    fn new() -> Self {
        Self {
            attempt: 0,
            next_backoff_ms: 0,
            last_initialize_failure_reason: None,
            consecutive_initialize_failures: 0,
        }
    }

    fn advance(&mut self) -> Duration {
        let cap = respawn_backoff_cap_ms();
        let backoff = if self.next_backoff_ms == 0 {
            RESPAWN_BACKOFF_INITIAL_MS.min(cap)
        } else {
            self.next_backoff_ms.min(cap)
        };
        self.next_backoff_ms = backoff.saturating_mul(2).min(cap);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_millis(backoff)
    }

    fn record_failure(&mut self, error: &AcpBootstrapError) {
        if error.code == ACP_ERROR_CODE_INITIALIZE_FAILED {
            if self.last_initialize_failure_reason.as_deref() == Some(error.reason.as_str()) {
                self.consecutive_initialize_failures =
                    self.consecutive_initialize_failures.saturating_add(1);
            } else {
                self.last_initialize_failure_reason = Some(error.reason.clone());
                self.consecutive_initialize_failures = 1;
            }
        } else {
            self.last_initialize_failure_reason = None;
            self.consecutive_initialize_failures = 0;
        }
    }

    fn should_give_up(&self) -> bool {
        self.consecutive_initialize_failures >= RESPAWN_INIT_FAILURE_THRESHOLD
    }

    fn reset_on_success(&mut self) {
        self.attempt = 0;
        self.next_backoff_ms = 0;
        self.last_initialize_failure_reason = None;
        self.consecutive_initialize_failures = 0;
    }
}
