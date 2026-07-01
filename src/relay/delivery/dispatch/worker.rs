use std::{sync::Arc, sync::OnceLock, time::Duration};

use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use tokio::{runtime::Handle, sync::mpsc::UnboundedReceiver, task::JoinSet};

use crate::{
    configuration::{BundleMember, SessionType},
    envelope::PromptBatchSettings,
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
};

use super::super::super::canonical_session_id;
use super::super::super::startup_state::note_session_served_successfully;
use super::super::super::stream::{
    RelayStreamEvent, StreamEventSendOutcome, broadcast_event_to_bundle_ui,
    list_registered_ui_sessions_for_bundle, send_event_to_registered_ui,
};
use super::super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, RelayError, SendOutcome, SendResult,
};
use super::super::async_worker::{
    AsyncWorkerKey, install_acp_worker_output_view, set_worker_readiness,
};
use super::super::choice_state::{
    ChoiceEventContext, build_acp_chooser, invalidate_pending_for_respawn,
};
use super::payload::{
    build_delivery_message, emit_envelope_metadata_inscription, resolve_target_member,
    target_is_relay_wide,
};
use crate::configuration::TargetConfiguration;
use crate::transports::{
    AcpDriverServices, ChoiceMade, ChoiceToMake, Chooser, DeliveryEnvelope, DeliveryMessage,
    OutcomeFuture, SingleDeliveryOutcome, StartupContext, TransportImpl, UiBroadcastStatus,
    UiIncomingMessage, UiOutcomePhase, UiTransportServices,
};

const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;

/// One in-flight write awaiting its transport [`OutcomeFuture`]. Carries the
/// originating task and whether a successful delivery should clear startup
/// failures (`true` for coder transports, `false` for UI), so the collect site
/// can map the resolved outcome onto a `SendResult` and complete the task. The
/// outcome is `None` if the future was dropped before resolving.
type InflightOutcome = (AsyncDeliveryTask, bool, Option<SingleDeliveryOutcome>);

#[derive(Clone)]
pub(super) struct AcpWorkerBootstrap {
    pub(super) target_member: BundleMember,
    pub(super) runtime_directory: std::path::PathBuf,
    /// Per-bundle choice-queue bound, captured into the chooser closure at worker
    /// construction so it no longer rides every delivery task and choice.
    pub(super) choices_pending_max: usize,
}

/// Spawns the per-target async delivery worker as a tokio task.
///
/// The worker runs a concurrent produce-and-collect loop: a `select!` over
/// `receiver.recv()` and a `JoinSet` of in-flight `OutcomeFuture`s submits
/// each task to its transport via the non-blocking `mailw`/`raww` seam and
/// collects the resolved outcomes. Blocking IO, quiescence/coalesce waits,
/// ACP bootstrap/respawn, and readiness mirroring all live inside the
/// transports' internal delivery tasks. Shutdown is observed via
/// `shutdown_requested()` polled between receives.
pub(super) fn spawn_async_delivery_worker(
    key: AsyncWorkerKey,
    receiver: UnboundedReceiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    bootstrap: Option<AcpWorkerBootstrap>,
) {
    delivery_runtime_handle().spawn(async move {
        run_async_delivery_worker(key, receiver, pending, bootstrap).await;
    });
}

/// Resolves the tokio runtime handle that hosts delivery worker tasks.
///
/// In production the relay binary runs under `#[tokio::main]`, so a current
/// runtime handle is always available and we reuse it. In CLI/test contexts
/// where workers are enqueued without an ambient runtime (one-shot
/// `request_relay` callers, startup helpers driven directly from sync
/// tests), a process-wide fallback multi-thread runtime is created on
/// demand. The fallback is multi-thread with a blocking pool because the
/// transports' internal delivery tasks submit blocking IO via
/// `spawn_blocking`.
fn delivery_runtime_handle() -> Handle {
    if let Ok(handle) = Handle::try_current() {
        return handle;
    }
    static DELIVERY_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    DELIVERY_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("agentmux-delivery")
                .build()
                .expect("build agentmux delivery fallback runtime")
        })
        .handle()
        .clone()
}

async fn run_async_delivery_worker(
    key: AsyncWorkerKey,
    mut receiver: UnboundedReceiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    bootstrap: Option<AcpWorkerBootstrap>,
) {
    // Hold one `TransportImpl` for this target's lifetime. The transport KIND is
    // the only target-type-dependent decision, and it is fixed at construction
    // from the configured `session_type()` (transport-abstraction spec): ACP
    // targets get the bootstrap driver here; every other target's transport is
    // built lazily from its first task (a non-ACP worker has no bundle member at
    // spawn time — the task carries it) and then latched. Delivery is uniform:
    // the loop submits `mailw`/`raww` for every target with no registry-based
    // re-routing and no transport-deliverability gate. ACP lifecycle, readiness
    // mirroring, and respawn live entirely in the driver and its internal task;
    // the loop never names an ACP type.
    //
    // The loop is a concurrent produce-and-collect: it submits each task to the
    // transport via the non-blocking `mailw`/`raww` seam and concurrently collects
    // the resolved `OutcomeFuture`s. Coalescing, quiescence, the token-budget
    // combine, and the blocking IO all live inside each transport's internal
    // delivery task now, so the worker no longer batches, hoists quiescence, or
    // owns `spawn_blocking`.
    let batch_settings = super::prompt_batch_settings();
    // `None` until the transport is constructed: eagerly for ACP (bootstrap),
    // lazily from the first task's `session_type()` for every other target.
    let mut transport: Option<TransportImpl> = match bootstrap {
        Some(bootstrap) => {
            let services = build_acp_driver_services(&key, &bootstrap);
            let mut transport = TransportImpl::acp(
                bootstrap.target_member,
                bootstrap.runtime_directory,
                key.namespace.clone(),
                services,
                batch_settings,
            );
            if let TransportImpl::Acp(driver) = &mut transport {
                driver.bootstrap().await;
            }
            Some(transport)
        }
        None => None,
    };
    let poll_interval = Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS);
    // In-flight writes: each entry awaits one transport `OutcomeFuture` and yields
    // its originating task so the collect arm can complete it. Completion order is
    // independent of submission order; FIFO ordering at the target is preserved by
    // the transport's internal channel, into which the produce arm enqueues in
    // receive order.
    let mut inflight: JoinSet<InflightOutcome> = JoinSet::new();
    let mut senders_dropped = false;

    loop {
        if shutdown_requested() {
            shutdown_drain(
                transport.as_mut(),
                &mut inflight,
                &mut receiver,
                pending.as_ref(),
            )
            .await;
            break;
        }
        if senders_dropped && inflight.is_empty() {
            // No more producers and nothing in flight: the worker is unreachable.
            break;
        }
        tokio::select! {
            maybe_task = receiver.recv(), if !senders_dropped => {
                match maybe_task {
                    Some(task) => {
                        if shutdown_requested() {
                            super::super::async_worker::complete_task_on_shutdown(&task);
                            super::super::async_worker::release_pending_slot(pending.as_ref());
                            continue;
                        }
                        submit_task(
                            task,
                            &key,
                            &mut transport,
                            batch_settings,
                            &mut inflight,
                            pending.as_ref(),
                        );
                    }
                    None => senders_dropped = true,
                }
            }
            joined = inflight.join_next(), if !inflight.is_empty() => {
                if let Some(joined) = joined {
                    collect_outcome(joined, pending.as_ref());
                }
            }
            _ = tokio::time::sleep(poll_interval) => {
                // Poll tick: re-evaluate the shutdown gate even while idle.
            }
        }
    }
    super::super::async_worker::unregister_worker(&key);
}

/// Submits one task to its transport via the non-blocking write seam and spawns
/// an in-flight collector for its outcome. On the worker's first task the
/// transport is constructed from the target's configured `session_type()` and
/// latched (`build_worker_transport`). Delivery is then uniform: `Ui` builds the
/// stream envelope, coder transports (ACP/Tmux) render the framed envelope or
/// submit raw input, and the forward-declared `Pubsub` stub yields an explicit
/// not-implemented outcome (it is not deliverable). A construction or render
/// failure completes the task immediately and releases its slot.
fn submit_task(
    task: AsyncDeliveryTask,
    key: &AsyncWorkerKey,
    transport: &mut Option<TransportImpl>,
    batch_settings: PromptBatchSettings,
    inflight: &mut JoinSet<InflightOutcome>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    if transport.is_none() {
        match build_worker_transport(&task, key, batch_settings) {
            Ok(built) => *transport = Some(built),
            Err(error) => {
                super::super::async_worker::complete_task_outcome(&task, Err(error));
                super::super::async_worker::release_pending_slot(pending);
                return;
            }
        }
    }
    let transport = transport
        .as_mut()
        .expect("worker transport constructed above");

    let (future, record_served) = if matches!(transport, TransportImpl::Pubsub) {
        // Forward-declared stub: not deliverable. Its `mailw`/`raww` are
        // `unimplemented!`, so produce an explicit terminal outcome instead of
        // calling them.
        super::super::async_worker::complete_task_outcome(
            &task,
            Err(super::super::super::session_type_not_implemented(
                task.target_session.as_str(),
                SessionType::Pubsub,
            )),
        );
        super::super::async_worker::release_pending_slot(pending);
        return;
    } else if matches!(transport, TransportImpl::Ui(_)) {
        (transport.mailw(build_ui_envelope(&task)), false)
    } else {
        match prepare_coder_write(&task, transport) {
            Ok(future) => (future, true),
            Err(error) => {
                super::super::async_worker::complete_task_outcome(&task, Err(error));
                super::super::async_worker::release_pending_slot(pending);
                return;
            }
        }
    };

    inflight.spawn(async move { (task, record_served, future.await.ok()) });
}

/// Constructs the worker's transport from the target's configured session type —
/// the only target-type-dependent step (transport-abstraction spec). Relay-wide
/// `@GLOBAL` targets have no bundle-member transport config and deliver via the UI
/// stream by principal, so they construct `UiTransport`. Configured members select
/// by `session_type()`: `Tmux` → `TmuxTransport` (with `startup()` so its internal
/// delivery task runs), `Ui` → `UiTransport`, `Pubsub` → the forward-declared stub.
/// ACP targets never reach here — they are constructed with a bootstrap driver,
/// and the enqueue path rejects ACP tasks unless that driver already exists.
fn build_worker_transport(
    task: &AsyncDeliveryTask,
    key: &AsyncWorkerKey,
    batch_settings: PromptBatchSettings,
) -> Result<TransportImpl, RelayError> {
    if target_is_relay_wide(task) {
        return Ok(TransportImpl::ui(build_ui_transport_services(key)));
    }
    let target_member =
        resolve_target_member(task)?.expect("configured non-relay-wide target must have a member");
    match target_member.target.session_type() {
        SessionType::Tmux => {
            let mut transport = TransportImpl::tmux(batch_settings);
            // tmux ignores the `choose` resolver (it raises no operator choices),
            // so a cancelling no-op chooser satisfies the `StartupContext` contract.
            let context = StartupContext {
                namespace: task.bundle.bundle_name.clone(),
                runtime_directory: task.runtime_directory.clone(),
                target_member: target_member.clone(),
                choose: noop_tmux_chooser(),
            };
            let _ = transport.startup(context);
            Ok(transport)
        }
        SessionType::Ui => Ok(TransportImpl::ui(build_ui_transport_services(key))),
        SessionType::Pubsub => Ok(TransportImpl::Pubsub),
        SessionType::Acp => Err(super::super::super::relay_error(
            "internal_unexpected_failure",
            "ACP target reached the non-bootstrap worker construction path",
            Some(json!({ "target_session": task.target_session })),
        )),
    }
}

/// Builds a coder task's structured payload and submits it via the non-blocking
/// write seam. Envelope-mode tasks build a [`DeliveryMessage`] (and emit the
/// out-of-band metadata inscription) then go through `mailw`, where the transport
/// renders its own pane envelope; raw-input tasks go through `raww` with the
/// task's `append_enter`.
fn prepare_coder_write(
    task: &AsyncDeliveryTask,
    transport: &mut TransportImpl,
) -> Result<OutcomeFuture, RelayError> {
    match task.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => {
            let target_member = resolve_target_member(task)?;
            let message = build_delivery_message(task, target_member, now_rfc3339().as_str());
            emit_envelope_metadata_inscription(&message, task.message_id.as_str());
            Ok(transport.mailw(build_coder_envelope(task, message)))
        }
        DeliveryPayloadMode::RawInput => {
            Ok(transport.raww(task.message.clone(), task.append_enter))
        }
    }
}

/// Maps one resolved in-flight outcome onto a `SendResult`, fans it back to the
/// originating sender, records `served_successfully` for delivered coder writes,
/// and releases the pending slot. A panicked collector task only releases the
/// slot (a panic is a bug, not a delivery result).
fn collect_outcome(
    joined: Result<InflightOutcome, tokio::task::JoinError>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    let (task, record_served, outcome) = match joined {
        Ok(value) => value,
        Err(_join_error) => {
            super::super::async_worker::release_pending_slot(pending);
            return;
        }
    };
    let send_result = match outcome {
        Some(outcome) => outcome_to_send_result(&task, outcome),
        None => dropped_send_result(&task),
    };
    if record_served && send_result.outcome == SendOutcome::Delivered {
        let _ = note_session_served_successfully(
            task.runtime_directory.as_path(),
            task.target_session.as_str(),
        );
    }
    super::super::async_worker::complete_task_outcome(&task, Ok(send_result));
    super::super::async_worker::release_pending_slot(pending);
}

/// Drains the worker on relay shutdown: signals the transport so its internal
/// delivery task resolves every in-flight write terminally, collects those
/// resolutions to completion, then drops the not-yet-submitted queued tasks. The
/// transport contract guarantees prompt terminal resolution on shutdown, so the
/// `join_next` drain does not park indefinitely. The transport is `None` if no
/// task ever arrived to construct it.
async fn shutdown_drain(
    transport: Option<&mut TransportImpl>,
    inflight: &mut JoinSet<InflightOutcome>,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    if let Some(transport) = transport {
        transport.shutdown();
    }
    while let Some(joined) = inflight.join_next().await {
        collect_outcome(joined, pending);
    }
    super::super::async_worker::drop_pending_async_tasks_on_shutdown(receiver, pending);
}

/// A cancelling no-op [`Chooser`] for the tmux `StartupContext`. Tmux never
/// raises operator choices, so this is never invoked; it exists only to satisfy
/// the contract's required resolver field.
fn noop_tmux_chooser() -> Chooser {
    Arc::new(|_choice: ChoiceToMake| ChoiceMade::Cancelled {
        decided_by: String::new(),
        reason_code: "choice_unsupported".to_string(),
        reason: Some("tmux transport does not raise operator choices".to_string()),
    })
}

/// Builds the [`DeliveryEnvelope`] for a coder (ACP/tmux) task from its structured
/// message. Envelope-mode writes always submit with Enter; the transport renders
/// the pane envelope from `message` before paste/turn submission.
///
/// The envelope's generic [`DeliveryEnvelope::prime_timeout_ms`] field is
/// populated from the per-coder config: `TmuxTargetConfiguration.prime_timeout_ms`
/// for tmux sessions (`[coders.<id>.tmux].prime-timeout-ms`) and
/// `AcpTargetConfiguration.prime_timeout_ms` for ACP sessions
/// (`[coders.<id>.acp].prime-timeout-ms`). Each transport consumes the same
/// generic envelope field; no transport-prefixed envelope field is introduced.
fn build_coder_envelope(task: &AsyncDeliveryTask, message: DeliveryMessage) -> DeliveryEnvelope {
    let prime_timeout_ms = match resolve_target_member(task) {
        Ok(Some(member)) => match &member.target {
            TargetConfiguration::Tmux(tmux_target) => tmux_target.prime_timeout_ms,
            TargetConfiguration::Acp(acp_target) => acp_target.prime_timeout_ms,
            TargetConfiguration::Ui | TargetConfiguration::Pubsub => None,
        },
        _ => None,
    };
    DeliveryEnvelope {
        message_id: task.message_id.clone(),
        message,
        append_enter: true,
        choice_decider_sessions: task.choice_decider_sessions.clone(),
        quiet_window: task.quiescence.quiet_window,
        prime_timeout_ms,
    }
}

/// Builds the relay lifecycle touchpoints the ACP worker driver invokes. Each
/// closure closes over this target's identity and the relay's own registries;
/// the driver holds them as opaque `Arc<dyn Fn>`s, so `src/acp` imports nothing
/// from `crate::relay`.
fn build_acp_driver_services(
    key: &AsyncWorkerKey,
    bootstrap: &AcpWorkerBootstrap,
) -> AcpDriverServices {
    let namespace = key.namespace.clone();
    let runtime_directory = bootstrap.runtime_directory.clone();
    let target_session = bootstrap.target_member.id.clone();
    let choices_pending_max = bootstrap.choices_pending_max;

    AcpDriverServices {
        mirror_state: {
            let namespace = namespace.clone();
            let runtime_directory = runtime_directory.clone();
            let target_session = target_session.clone();
            Arc::new(move |state| {
                set_worker_readiness(
                    namespace.as_str(),
                    runtime_directory.as_path(),
                    target_session.as_str(),
                    state,
                );
            })
        },
        publish_output: {
            let namespace = namespace.clone();
            let runtime_directory = runtime_directory.clone();
            let target_session = target_session.clone();
            Arc::new(move |output_view| {
                install_acp_worker_output_view(
                    namespace.as_str(),
                    runtime_directory.as_path(),
                    target_session.as_str(),
                    output_view,
                );
            })
        },
        broadcast_ui: {
            let namespace = namespace.clone();
            let target_session = target_session.clone();
            Arc::new(move |event_type: &str, payload| {
                broadcast_event_to_bundle_ui(
                    namespace.as_str(),
                    &acp_respawn_stream_event(
                        event_type,
                        namespace.as_str(),
                        target_session.as_str(),
                        payload,
                    ),
                );
            })
        },
        invalidate_choices: {
            let namespace = namespace.clone();
            let runtime_directory = runtime_directory.clone();
            let target_session = target_session.clone();
            Arc::new(move || {
                let context = ChoiceEventContext {
                    runtime_directory: runtime_directory.clone(),
                    namespace: namespace.clone(),
                    authorized_ui_sessions: list_registered_ui_sessions_for_bundle(
                        namespace.as_str(),
                    ),
                };
                if let Err(reason) =
                    invalidate_pending_for_respawn(&context, target_session.as_str())
                {
                    emit_inscription(
                        "relay.acp.respawn.choice_invalidate_failed",
                        &json!({
                            "namespace": namespace,
                            "target_session": target_session,
                            "reason": reason,
                        }),
                    );
                }
            })
        },
        chooser: build_acp_chooser(namespace, runtime_directory, choices_pending_max),
    }
}

/// Builds the UI broadcast touchpoints the `UiTransport` invokes. Each closure
/// closes over this target's `(namespace, target_session)` and the relay's own
/// stream registry; the transport holds them as opaque `Arc<dyn Fn>`s, so
/// `src/transports` imports nothing from `crate::relay` (mirrors
/// `build_acp_driver_services`).
fn build_ui_transport_services(key: &AsyncWorkerKey) -> UiTransportServices {
    let namespace = key.namespace.clone();
    let target_session = key.target_session.clone();
    UiTransportServices {
        broadcast_incoming: {
            let namespace = namespace.clone();
            let target_session = target_session.clone();
            Arc::new(move |incoming: &UiIncomingMessage| {
                let mut payload = json!({
                    "message_id": incoming.message_id,
                    "sender_session": incoming.sender_session,
                    "body": incoming.body,
                    "cc_sessions": if incoming.cc_sessions.is_empty() {
                        Value::Null
                    } else {
                        json!(incoming.cc_sessions)
                    },
                });
                if let Some(authenticated_identity) = &incoming.authenticated_identity {
                    payload["authenticated_identity"] =
                        Value::String(authenticated_identity.clone());
                }
                let event = RelayStreamEvent {
                    event_type: "incoming_message".to_string(),
                    target_session: canonical_session_id(
                        target_session.as_str(),
                        namespace.as_str(),
                    ),
                    created_at: now_rfc3339(),
                    payload,
                };
                stream_send_to_broadcast_status(send_event_to_registered_ui(
                    namespace.as_str(),
                    target_session.as_str(),
                    &event,
                ))
            })
        },
        emit_phase: {
            let namespace = namespace.clone();
            let target_session = target_session.clone();
            Arc::new(move |phase: UiOutcomePhase| {
                let mut payload = serde_json::Map::new();
                payload.insert("message_id".to_string(), Value::String(phase.message_id));
                payload.insert("phase".to_string(), Value::String(phase.phase.to_string()));
                payload.insert(
                    "outcome".to_string(),
                    phase
                        .outcome
                        .map(|value| Value::String(value.to_string()))
                        .unwrap_or(Value::Null),
                );
                if let Some(reason_code) = phase.reason_code {
                    payload.insert("reason_code".to_string(), Value::String(reason_code));
                }
                if let Some(reason) = phase.reason {
                    payload.insert("reason".to_string(), Value::String(reason));
                }
                let event = RelayStreamEvent {
                    event_type: "delivery_outcome".to_string(),
                    target_session: canonical_session_id(
                        target_session.as_str(),
                        namespace.as_str(),
                    ),
                    created_at: now_rfc3339(),
                    payload: Value::Object(payload),
                };
                stream_send_to_broadcast_status(send_event_to_registered_ui(
                    namespace.as_str(),
                    target_session.as_str(),
                    &event,
                ))
            })
        },
    }
}

/// Maps a relay stream-send result onto the transport-side [`UiBroadcastStatus`],
/// keeping the relay's `StreamEventSendOutcome` taxonomy out of `transports`.
fn stream_send_to_broadcast_status(
    result: Result<StreamEventSendOutcome, std::io::Error>,
) -> UiBroadcastStatus {
    match result {
        Ok(StreamEventSendOutcome::Delivered) => UiBroadcastStatus::Delivered,
        Ok(StreamEventSendOutcome::NoUiEndpoint | StreamEventSendOutcome::Disconnected) => {
            UiBroadcastStatus::NoUi
        }
        Err(source) => {
            UiBroadcastStatus::Failed(format!("failed to emit relay stream event: {source}"))
        }
    }
}

/// Builds the [`DeliveryEnvelope`] for a UI-routed task from the same structured
/// [`DeliveryMessage`] coder transports receive; the UI transport reads its
/// attribution fields to build the `incoming_message` stream event instead of
/// rendering pane text. The UI transport owns its own reconnect cap
/// ([`UI_RECONNECT_TIMEOUT_MS_DEFAULT`](crate::transports::ui::UI_RECONNECT_TIMEOUT_MS_DEFAULT));
/// the relay no longer threads an external knob through the envelope.
fn build_ui_envelope(task: &AsyncDeliveryTask) -> DeliveryEnvelope {
    let target_member = task
        .bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session);
    let message = build_delivery_message(task, target_member, now_rfc3339().as_str());
    DeliveryEnvelope {
        message_id: task.message_id.clone(),
        message,
        append_enter: task.append_enter,
        choice_decider_sessions: task.choice_decider_sessions.clone(),
        quiet_window: task.quiescence.quiet_window,
        prime_timeout_ms: None,
    }
}

/// Maps a transport outcome onto the relay `SendResult`, substituting the task's
/// own correlation fields (the transport leaves them blank; the relay is
/// authoritative for them). Shared by every transport — the worker dispatches
/// `mailw`/`raww` uniformly, so the collect site maps outcomes uniformly too.
fn outcome_to_send_result(task: &AsyncDeliveryTask, outcome: SingleDeliveryOutcome) -> SendResult {
    SendResult {
        target_session: task.target_session.clone(),
        message_id: task.message_id.clone(),
        outcome: outcome.outcome,
        reason_code: outcome.reason_code,
        reason: outcome.reason,
        details: outcome.details,
    }
}

/// Result for a task whose outcome future was dropped before resolving (the
/// transport's delivery task vanished); treated as a shutdown drop.
fn dropped_send_result(task: &AsyncDeliveryTask) -> SendResult {
    SendResult {
        target_session: task.target_session.clone(),
        message_id: task.message_id.clone(),
        outcome: SendOutcome::DroppedOnShutdown,
        reason_code: Some("dropped_on_shutdown".to_string()),
        reason: Some("delivery worker dropped before completion".to_string()),
        details: None,
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn acp_respawn_stream_event(
    event_type: &str,
    namespace: &str,
    target_session: &str,
    payload: serde_json::Value,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: event_type.to_string(),
        target_session: canonical_session_id(target_session, namespace),
        created_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        payload,
    }
}
