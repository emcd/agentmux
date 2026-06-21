use std::{sync::Arc, sync::OnceLock, time::Duration};

use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use tokio::{runtime::Handle, sync::mpsc::UnboundedReceiver, task::JoinSet};

use crate::{
    configuration::{BundleMember, SessionType},
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
    AsyncWorkerKey, install_acp_worker_output_view, set_acp_worker_state,
};
use super::super::choice_state::{
    ChoiceEventContext, build_acp_chooser, invalidate_pending_for_respawn,
};
use super::super::quiescence::QUIESCENCE_TIMEOUT_MS_DEFAULT;
use super::payload::{co_recipient_sessions, render_task_envelope, resolve_target_member};
use crate::transports::{
    AcpDriverServices, ChoiceMade, ChoiceToMake, Chooser, DeliveryEnvelope, OutcomeFuture,
    SingleDeliveryOutcome, StartupContext, TransportImpl, UiBroadcastStatus, UiIncomingMessage,
    UiOutcomePhase, UiTransportServices,
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
/// The worker awaits delivery tasks on a `tokio::sync::mpsc::UnboundedReceiver`
/// and offloads the synchronous ACP / tmux delivery body to `spawn_blocking`,
/// so the tokio runtime worker thread is not pinned during the IO. ACP
/// bootstrap, respawn, and the per-target single-flight wait for prompt
/// completion are likewise offloaded to blocking tasks. Shutdown is observed
/// via `shutdown_requested()` polled between receives.
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
/// In production the relay binary runs under `#[tokio::main]` and worker
/// enqueue happens inside `spawn_blocking` from the relay accept loop or
/// inline from an async stream handler, so a current runtime handle is
/// always available and we reuse it. In CLI/test contexts where workers are
/// enqueued without an ambient runtime (one-shot `request_relay` callers,
/// startup helpers driven directly from sync tests), a process-wide
/// fallback multi-thread runtime is created on demand. Both surfaces give
/// the worker the multi-thread + blocking pool flavor it needs for the
/// `spawn_blocking` calls inside the task body.
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
    let max_prompt_tokens = super::prompt_batch_settings().max_prompt_tokens;
    // `None` until the transport is constructed: eagerly for ACP (bootstrap),
    // lazily from the first task's `session_type()` for every other target.
    let mut transport: Option<TransportImpl> = match bootstrap {
        Some(bootstrap) => {
            let services = build_acp_driver_services(&key, &bootstrap);
            let mut transport = TransportImpl::acp(
                bootstrap.target_member,
                bootstrap.runtime_directory,
                key.bundle_name.clone(),
                services,
                max_prompt_tokens,
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
                            max_prompt_tokens,
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
    max_prompt_tokens: usize,
    inflight: &mut JoinSet<InflightOutcome>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    if transport.is_none() {
        match build_worker_transport(&task, key, max_prompt_tokens) {
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
    max_prompt_tokens: usize,
) -> Result<TransportImpl, RelayError> {
    if task.relay_wide_target {
        return Ok(TransportImpl::ui(build_ui_transport_services(key)));
    }
    let target_member =
        resolve_target_member(task)?.expect("configured non-relay-wide target must have a member");
    match target_member.target.session_type() {
        SessionType::Tmux => {
            let mut transport = TransportImpl::tmux(max_prompt_tokens);
            // tmux ignores the `choose` resolver (it raises no operator choices),
            // so a cancelling no-op chooser satisfies the `StartupContext` contract.
            let context = StartupContext {
                bundle_name: task.bundle.bundle_name.clone(),
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

/// Renders a coder task and submits it via the non-blocking write seam.
/// Envelope-mode tasks render their framed envelope and go through `mailw`;
/// raw-input tasks go through `raww` with the task's `append_enter`.
fn prepare_coder_write(
    task: &AsyncDeliveryTask,
    transport: &mut TransportImpl,
) -> Result<OutcomeFuture, RelayError> {
    match task.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => {
            let target_member = resolve_target_member(task)?;
            let rendered = render_task_envelope(task, target_member, now_rfc3339().as_str());
            Ok(transport.mailw(build_coder_envelope(task, rendered)))
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

/// Builds the [`DeliveryEnvelope`] for a coder (ACP/tmux) task from its rendered
/// envelope text. Envelope-mode writes always submit with Enter. The R1 UI
/// attribution fields are blank — coder transports ignore them.
fn build_coder_envelope(task: &AsyncDeliveryTask, rendered: String) -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: task.message_id.clone(),
        payload_mode: task.payload_mode,
        rendered,
        append_enter: true,
        choice_decider_sessions: task.choice_decider_sessions.clone(),
        quiet_window: task.quiescence.quiet_window,
        quiescence_timeout: task.quiescence.quiescence_timeout,
        sender_session: String::new(),
        cc_sessions: Vec::new(),
        authenticated_identity: None,
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
    let bundle_name = key.bundle_name.clone();
    let runtime_directory = bootstrap.runtime_directory.clone();
    let target_session = bootstrap.target_member.id.clone();
    let choices_pending_max = bootstrap.choices_pending_max;

    AcpDriverServices {
        mirror_state: {
            let bundle_name = bundle_name.clone();
            let runtime_directory = runtime_directory.clone();
            let target_session = target_session.clone();
            Arc::new(move |state| {
                set_acp_worker_state(
                    bundle_name.as_str(),
                    runtime_directory.as_path(),
                    target_session.as_str(),
                    state,
                );
            })
        },
        publish_output: {
            let bundle_name = bundle_name.clone();
            let runtime_directory = runtime_directory.clone();
            let target_session = target_session.clone();
            Arc::new(move |output_view| {
                install_acp_worker_output_view(
                    bundle_name.as_str(),
                    runtime_directory.as_path(),
                    target_session.as_str(),
                    output_view,
                );
            })
        },
        broadcast_ui: {
            let bundle_name = bundle_name.clone();
            let target_session = target_session.clone();
            Arc::new(move |event_type: &str, payload| {
                broadcast_event_to_bundle_ui(
                    bundle_name.as_str(),
                    &acp_respawn_stream_event(
                        event_type,
                        bundle_name.as_str(),
                        target_session.as_str(),
                        payload,
                    ),
                );
            })
        },
        invalidate_choices: {
            let bundle_name = bundle_name.clone();
            let runtime_directory = runtime_directory.clone();
            let target_session = target_session.clone();
            Arc::new(move || {
                let context = ChoiceEventContext {
                    runtime_directory: runtime_directory.clone(),
                    bundle_name: bundle_name.clone(),
                    authorized_ui_sessions: list_registered_ui_sessions_for_bundle(
                        bundle_name.as_str(),
                    ),
                };
                if let Err(reason) =
                    invalidate_pending_for_respawn(&context, target_session.as_str())
                {
                    emit_inscription(
                        "relay.acp.respawn.choice_invalidate_failed",
                        &json!({
                            "bundle_name": bundle_name,
                            "target_session": target_session,
                            "reason": reason,
                        }),
                    );
                }
            })
        },
        chooser: build_acp_chooser(bundle_name, runtime_directory, choices_pending_max),
    }
}

/// Builds the UI broadcast touchpoints the `UiTransport` invokes. Each closure
/// closes over this target's `(bundle_name, target_session)` and the relay's own
/// stream registry; the transport holds them as opaque `Arc<dyn Fn>`s, so
/// `src/transports` imports nothing from `crate::relay` (mirrors
/// `build_acp_driver_services`).
fn build_ui_transport_services(key: &AsyncWorkerKey) -> UiTransportServices {
    let bundle_name = key.bundle_name.clone();
    let target_session = key.target_session.clone();
    UiTransportServices {
        broadcast_incoming: {
            let bundle_name = bundle_name.clone();
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
                        bundle_name.as_str(),
                    ),
                    created_at: now_rfc3339(),
                    payload,
                };
                stream_send_to_broadcast_status(send_event_to_registered_ui(
                    bundle_name.as_str(),
                    target_session.as_str(),
                    &event,
                ))
            })
        },
        emit_phase: {
            let bundle_name = bundle_name.clone();
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
                        bundle_name.as_str(),
                    ),
                    created_at: now_rfc3339(),
                    payload: Value::Object(payload),
                };
                stream_send_to_broadcast_status(send_event_to_registered_ui(
                    bundle_name.as_str(),
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

/// Builds the [`DeliveryEnvelope`] for a UI-routed task. `rendered` carries the
/// raw message body (the UI renders its own framing, unlike coder transports);
/// the R1 attribution fields are relay-populated and read only by the UI
/// transport. `quiescence_timeout` is resolved here so the transport's reconnect
/// cap matches the relay quiescence default.
fn build_ui_envelope(task: &AsyncDeliveryTask) -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: task.message_id.clone(),
        payload_mode: task.payload_mode,
        rendered: task.message.clone(),
        append_enter: task.append_enter,
        choice_decider_sessions: task.choice_decider_sessions.clone(),
        quiet_window: task.quiescence.quiet_window,
        quiescence_timeout: Some(
            task.quiescence
                .quiescence_timeout
                .unwrap_or(Duration::from_millis(QUIESCENCE_TIMEOUT_MS_DEFAULT)),
        ),
        sender_session: canonical_session_id(
            task.sender.id.as_str(),
            task.sender_bundle_name.as_str(),
        ),
        cc_sessions: co_recipient_sessions(task),
        authenticated_identity: task.authenticated_identity.clone(),
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
    bundle_name: &str,
    target_session: &str,
    payload: serde_json::Value,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: event_type.to_string(),
        target_session: canonical_session_id(target_session, bundle_name),
        created_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        payload,
    }
}
