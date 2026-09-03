//! Envelope and service builders used by the async-delivery worker.
//!
//! Five free fns extracted from `worker.rs`:
//! - `build_worker_transport` constructs the per-task `TransportImpl` and runs
//!   its `startup` for tmux/Pty surfaces, off the async runtime via
//!   `start_transport_off_runtime`.
//! - `build_coder_envelope`/`build_ui_envelope` build the coder vs. UI
//!   `DeliveryEnvelope`s from an `AsyncDeliveryTask`.
//! - `build_acp_driver_services`/`build_ui_transport_services` close over
//!   the relay-internal lifecycle touchpoints the ACP and UI transport
//!   drivers invoke (record-failure / mirror-state / publish-output / chooser
//!   / broadcast). These are opaque `Arc<dyn Fn>`s so `src/acp` and
//!   `src/transports` don't pull anything from `crate::relay`.
//!
//! `noop_tmux_chooser` is a private helper of `build_worker_transport` kept
//! here so the transport builder is self-contained.

use std::sync::Arc;

use serde_json::{Value, json};

use super::outcomes;
use super::payload::build_delivery_message;
use super::worker::{AcpWorkerBootstrap, WorkerTransportContext};

use crate::relay::delivery::async_worker::{
    AsyncWorkerKey, install_acp_worker_output_view, set_worker_failure, set_worker_readiness,
};
use crate::relay::delivery::choice_state::{
    ChoiceEventContext, build_acp_chooser, invalidate_pending_for_respawn,
};
use crate::relay::stream::{
    RelayStreamEvent, StreamEventSendOutcome, broadcast_event_to_bundle_ui,
    list_registered_ui_sessions_for_bundle, send_event_to_registered_ui,
};
use crate::relay::{
    AsyncDeliveryTask, RelayError, errors::relay_error, identity::canonical_session_id,
};

use crate::configuration::{BundleMember, SessionType};
use crate::envelope::PromptBatchSettings;
use crate::runtime::inscriptions::emit_inscription;
use crate::transports::{
    AcpDriverServices, ChoiceMade, ChoiceToMake, Chooser, DeliveryEnvelope,
    DeliveryExecutorContext, DeliveryMessage, StartupContext, TransportError, TransportImpl,
    UiBroadcastStatus, UiIncomingMessage, UiOutcomePhase, UiTransportServices,
};

/// Builds the [`TransportImpl`] for a non-bootstrap worker delivery, dispatching
/// on `task`'s target `SessionType`. Tmux/Pty surfaces run a cooperative
/// `startup` so the relay's `StartupContext` contract is satisfied; UI surfaces
/// receive a `UiTransport` built via `build_ui_transport_services`; Pubsub and
/// Acp variants carry their own forwarding/bootstrap path (the enqueue layer
/// rejects ACP unless a bootstrap driver already exists).
/// Maps a transport's startup failure onto the relay error the triggering task
/// resolves with.
///
/// Carries the transport's own code and details through: the reason a pty child
/// failed to spawn is the useful part, and flattening it to a generic failure at
/// the relay boundary is what left a construction error indistinguishable from a
/// target that merely went quiet.
fn startup_failure(target_session: &str, error: TransportError) -> RelayError {
    relay_error(
        "runtime_transport_startup_failed",
        error.reason.as_str(),
        Some(json!({
            "target_session": target_session,
            "code": error.code,
            "details": error.details,
        })),
    )
}

/// Runs a transport's `startup` off the async runtime and returns the started
/// transport.
///
/// The single place the relay invokes [`Transport::startup`], so the rule it
/// enforces needs no per-transport judgement: `startup` is a *synchronous*
/// method on the delivery contract, which makes every implementation of it a
/// blocking call, and a blocking call on a runtime worker thread starves
/// everything else that thread owes progress to. Deciding that per session type
/// -- pty spawns a process so it goes off-runtime, tmux only spawns threads so
/// it stays -- would be reasoning from what the implementations happen to do
/// today rather than from what the contract permits them to do, and the next
/// implementation to acquire a blocking step would inherit a decision nobody
/// revisits.
///
/// `spawn_blocking` tasks cannot be aborted, so the closure owns everything it
/// creates: a startup that is still running when its awaiting task goes away
/// must reach its own conclusion and clean up after itself. Both transports do,
/// each through a guard whose `Drop` reclaims the partial runtime.
/// Starts a transport whose `startup` performs no blocking work, on the caller's
/// own thread.
///
/// The point is not to save a task switch. A `spawn_blocking` join is a
/// dependency on the runtime living long enough to resolve it, and a worker
/// cancelled at that await has already claimed its target's consumer generation
/// — so the mailbox is left with a generation that holds it and no executor that
/// will ever peek it. Starting inline removes the window entirely for the one
/// transport that has nothing to block on.
fn start_transport_inline(
    mut transport: TransportImpl,
    context: StartupContext,
    target_session: String,
) -> Result<TransportImpl, RelayError> {
    match transport.startup(context) {
        Ok(_) => Ok(transport),
        Err(error) => Err(startup_failure(target_session.as_str(), error)),
    }
}

async fn start_transport_off_runtime(
    mut transport: TransportImpl,
    context: StartupContext,
    target_session: String,
) -> Result<TransportImpl, RelayError> {
    tokio::task::spawn_blocking(move || match transport.startup(context) {
        Ok(_) => Ok(transport),
        Err(error) => Err(startup_failure(target_session.as_str(), error)),
    })
    .await
    .map_err(|join_error| {
        relay_error(
            "runtime_transport_startup_failed",
            "transport startup panicked",
            Some(json!({ "details": join_error.to_string() })),
        )
    })?
}

/// Builds a non-ACP worker's transport from inputs resolved before the worker
/// existed.
///
/// Takes a [`WorkerTransportContext`] rather than a delivery task because
/// construction is a property of the *target*, not of whichever message happened
/// to arrive first. Reading it off a task made that distinction unrepresentable
/// and deferred every construction failure past the point where the spawn site
/// could still report it to the sender synchronously.
pub(super) async fn build_worker_transport(
    context: &WorkerTransportContext,
    key: &AsyncWorkerKey,
    batch_settings: PromptBatchSettings,
    readiness_notifier: crate::tmux::ReadinessNotifier,
    delivery: DeliveryExecutorContext,
) -> Result<TransportImpl, RelayError> {
    // Every transport is started, including the two that used to skip `startup`
    // entirely: `startup` is where a transport spawns its delivery-loop executor,
    // so a transport the relay never started is one whose target nothing
    // consumes.
    //
    // Where that start happens differs, and the discriminator is whether the
    // start *blocks*. `spawn_blocking` exists to keep blocking IO off a runtime
    // thread; it also makes the start depend on the runtime surviving long enough
    // to resolve the join. tmux, pty and ACP open panes, spawn children and wait
    // on them, so they pay that dependency for a reason. A UI target has no
    // external resource to open at all — it is a broadcast surface over the
    // relay's own registry — so putting its start behind a join buys nothing and
    // costs the one thing that matters here: a worker cancelled while awaiting it
    // leaves the target with a mailbox nothing will ever consume, and no executor
    // to notice.
    let startup = |target_member: &BundleMember| StartupContext {
        namespace: context.namespace.clone(),
        runtime_directory: context.runtime_directory.clone(),
        target_member: target_member.clone(),
        // tmux, pty and UI ignore the `choose` resolver (none raises operator
        // choices), so a cancelling no-op satisfies the contract's required
        // resolver field.
        choose: noop_tmux_chooser(),
    };
    let Some(target_member) = context.target_member.as_ref() else {
        // No bundle member is the relay-wide (UI) target: `WorkerTransportContext`
        // only resolves to `None` for one, since a configured coder target with no
        // member fails resolution outright.
        return start_transport_inline(
            TransportImpl::ui(build_ui_transport_services(key), delivery),
            startup(&relay_wide_member(key)),
            context.target_session.clone(),
        );
    };
    match target_member.target.session_type() {
        SessionType::Tmux => {
            let transport = TransportImpl::tmux(batch_settings, Some(readiness_notifier), delivery);
            start_transport_off_runtime(
                transport,
                startup(target_member),
                context.target_session.clone(),
            )
            .await
        }
        SessionType::Ui => start_transport_inline(
            TransportImpl::ui(build_ui_transport_services(key), delivery),
            startup(target_member),
            context.target_session.clone(),
        ),
        SessionType::Pubsub => Ok(TransportImpl::Pubsub),
        #[cfg(feature = "pty")]
        SessionType::Pty => {
            // Pty target reached the non-bootstrap worker construction path.
            // The per-coder configuration is read from the resolved target; the
            // fallback arm below covers a member whose target is not a Pty
            // configuration, which the session-type dispatch above should already
            // have excluded.
            let pty_config = match &target_member.target {
                crate::configuration::TargetConfiguration::Pty(pty_cfg) => {
                    crate::pty::PtyTargetConfiguration {
                        initial_command: pty_cfg.initial_command.clone(),
                        resume_command: pty_cfg.resume_command.clone(),
                        prompt_readiness: pty_cfg.prompt_readiness.clone(),
                        cols: pty_cfg.cols,
                        rows: pty_cfg.rows,
                        working_directory: target_member.working_directory.clone(),
                        term_protocol: pty_cfg.term_protocol,
                    }
                }
                _ => crate::pty::PtyTargetConfiguration {
                    initial_command: String::new(),
                    resume_command: String::new(),
                    prompt_readiness: None,
                    cols: 120,
                    rows: 40,
                    working_directory: target_member.working_directory.clone(),
                    term_protocol: crate::configuration::TermProtocol::default(),
                },
            };
            // Mirror ACP's `MirrorStateFn` pattern (see
            // `build_acp_driver_services` above): the relay constructs
            // the closure closing over
            // `set_worker_readiness(namespace, runtime_directory,
            // target_session, state)`. The transport holds an opaque
            // `Arc<dyn Fn>` so `src/pty` does not import `crate::relay`.
            // The transport uses the closure to publish
            // `Initializing` / `Available` / `Busy` / `Unavailable`
            // transitions into the relay's global worker-state
            // registry at its lifecycle points.
            let pty_mirror_state: crate::pty::PtyMirrorStateFn = {
                let namespace = context.namespace.clone();
                let runtime_directory = context.runtime_directory.clone();
                let target_session = target_member.id.clone();
                Arc::new(move |state| {
                    set_worker_readiness(
                        namespace.as_str(),
                        runtime_directory.as_path(),
                        target_session.as_str(),
                        state,
                    );
                })
            };
            let transport = TransportImpl::pty(
                target_member.clone(),
                pty_config,
                Some(pty_mirror_state),
                delivery,
            );
            // Propagated rather than discarded. A dropped startup error installed
            // an already-dead transport whose executor would peek a mailbox it
            // could not write, so the entries waited out the whole dwell before
            // anything reported a generic unreachable -- turning a spawn failure
            // the relay already knew about into a delayed guess.
            start_transport_off_runtime(
                transport,
                startup(target_member),
                context.target_session.clone(),
            )
            .await
        }
        #[cfg(not(feature = "pty"))]
        SessionType::Pty => Err(relay_error(
            "internal_unexpected_failure",
            "PTY target configured but pty Cargo feature is disabled",
            Some(json!({ "target_session": context.target_session })),
        )),
        SessionType::Acp => Err(relay_error(
            "internal_unexpected_failure",
            "ACP target reached the non-bootstrap worker construction path",
            Some(json!({ "target_session": context.target_session })),
        )),
    }
}

/// The synthetic member a relay-wide (UI) target is started against.
///
/// A relay-wide principal has no bundle member — that is what makes it
/// relay-wide — but `startup` takes one, and the UI transport reads nothing from
/// it: it has no runtime to establish, no pane to size, and no command to spawn.
/// Naming the target is what the field is for here, so the stub carries that and
/// nothing else.
fn relay_wide_member(key: &AsyncWorkerKey) -> BundleMember {
    BundleMember {
        id: key.target_session.clone(),
        name: None,
        working_directory: None,
        target: crate::configuration::TargetConfiguration::Ui,
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    }
}

/// A cancelling no-op [`Chooser`] for a `StartupContext` whose transport raises
/// no operator choices. Never invoked; it exists only to satisfy the contract's
/// required resolver field.
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
pub(super) fn build_coder_envelope(
    task: &AsyncDeliveryTask,
    message: DeliveryMessage,
) -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: task.message_id.clone(),
        message,
        append_enter: true,
        choice_decider_sessions: task.choice_decider_sessions.clone(),
        is_receipt: task.is_receipt,
    }
}

/// Builds the relay lifecycle touchpoints the ACP worker driver invokes. Each
/// closure closes over this target's identity and the relay's own registries;
/// the driver holds them as opaque `Arc<dyn Fn>`s, so `src/acp` imports nothing
/// from `crate::relay`.
pub(super) fn build_acp_driver_services(
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
        record_failure: {
            let namespace = namespace.clone();
            let runtime_directory = runtime_directory.clone();
            let target_session = target_session.clone();
            Arc::new(move |failure| {
                set_worker_failure(
                    namespace.as_str(),
                    runtime_directory.as_path(),
                    target_session.as_str(),
                    failure,
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
            Arc::new(move |event_type: &str, payload: Value| {
                broadcast_event_to_bundle_ui(
                    namespace.as_str(),
                    &outcomes::acp_respawn_stream_event(
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
pub(super) fn build_ui_transport_services(key: &AsyncWorkerKey) -> UiTransportServices {
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
                if let Some(on_behalf_of) = &incoming.on_behalf_of {
                    payload["on_behalf_of"] = Value::String(on_behalf_of.clone());
                }
                let event = RelayStreamEvent {
                    event_type: "incoming_message".to_string(),
                    target_session: canonical_session_id(
                        target_session.as_str(),
                        namespace.as_str(),
                    ),
                    created_at: outcomes::now_rfc3339(),
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
                    created_at: outcomes::now_rfc3339(),
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

/// Builds the [`DeliveryEnvelope`] for a UI-routed task from the same structured
/// [`DeliveryMessage`] coder transports receive; the UI transport reads its
/// attribution fields to build the `incoming_message` stream event instead of
/// rendering pane text. No timing knob is threaded through the envelope, and the
/// UI transport no longer keeps one of its own: a delivery resolves from the one
/// broadcast it attempts.
///
/// `created_at` is supplied rather than read here. Both delivery directions stamp
/// one entry once, at the point its payload is built and placed in the mailbox, so
/// a builder that read the clock itself would restamp whatever it was asked to
/// render — and the mailbox would then hold an envelope carrying a different
/// `Date` from the one that reached the target.
pub(super) fn build_ui_envelope(task: &AsyncDeliveryTask, created_at: &str) -> DeliveryEnvelope {
    let target_member = task
        .bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session);
    let message = build_delivery_message(task, target_member, created_at);
    DeliveryEnvelope {
        message_id: task.message_id.clone(),
        message,
        append_enter: task.append_enter,
        choice_decider_sessions: task.choice_decider_sessions.clone(),
        is_receipt: task.is_receipt,
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
