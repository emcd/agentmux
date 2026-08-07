//! Envelope and service builders used by the async-delivery worker.
//!
//! Five free fns extracted from `worker.rs`:
//! - `build_worker_transport` constructs the per-task `TransportImpl` and runs
//!   its `startup` for tmux/Pty surfaces, off the async runtime via
//!   `start_transport_off_runtime`.
//! - `build_coder_envelope`/`build_ui_envelope` build the coder vs. UI
//!   `DeliveryEnvelope`s from an `AsyncDeliveryTask`, including the per-coder
//!   prime-timeout carry-over.
//! - `build_acp_driver_services`/`build_ui_transport_services` close over
//!   the relay-internal lifecycle touchpoints the ACP and UI transport
//!   drivers invoke (record-failure / mirror-state / publish-output / chooser
//!   / broadcast). These are opaque `Arc<dyn Fn>`s so `src/acp` and
//!   `src/transports` don't pull anything from `crate::relay`.
//!
//! `noop_tmux_chooser` is a private helper of `build_worker_transport` kept
//! here so the transport builder is self-contained.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::outcomes;
use super::payload::{build_delivery_message, resolve_target_member, target_is_relay_wide};
use super::worker::AcpWorkerBootstrap;

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

use crate::configuration::{SessionType, TargetConfiguration};
use crate::envelope::PromptBatchSettings;
use crate::runtime::inscriptions::emit_inscription;
use crate::transports::{
    AcpDriverServices, ChoiceMade, ChoiceToMake, Chooser, DeliveryEnvelope, DeliveryMessage,
    StartupContext, TransportError, TransportImpl, UiBroadcastStatus, UiIncomingMessage,
    UiOutcomePhase, UiTransportServices,
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

pub(super) async fn build_worker_transport(
    task: &AsyncDeliveryTask,
    key: &AsyncWorkerKey,
    batch_settings: PromptBatchSettings,
    readiness_notifier: crate::tmux::ReadinessNotifier,
) -> Result<TransportImpl, RelayError> {
    if target_is_relay_wide(task) {
        return Ok(TransportImpl::ui(build_ui_transport_services(key)));
    }
    let target_member =
        resolve_target_member(task)?.expect("configured non-relay-wide target must have a member");
    match target_member.target.session_type() {
        SessionType::Tmux => {
            let transport = TransportImpl::tmux(batch_settings, Some(readiness_notifier));
            // tmux ignores the `choose` resolver (it raises no operator choices),
            // so a cancelling no-op chooser satisfies the `StartupContext` contract.
            let context = StartupContext {
                namespace: task.bundle.bundle_name.clone(),
                runtime_directory: task.runtime_directory.clone(),
                target_member: target_member.clone(),
                choose: noop_tmux_chooser(),
            };
            start_transport_off_runtime(transport, context, task.target_session.clone()).await
        }
        SessionType::Ui => Ok(TransportImpl::ui(build_ui_transport_services(key))),
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
                        prime_timeout_ms: pty_cfg.prime_timeout_ms,
                        wedge_detection: pty_cfg.wedge_detection,
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
                    prime_timeout_ms: None,
                    wedge_detection: true,
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
                let namespace = task.bundle.bundle_name.clone();
                let runtime_directory = task.runtime_directory.clone();
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
            let transport =
                TransportImpl::pty(target_member.clone(), pty_config, Some(pty_mirror_state));
            let context = StartupContext {
                namespace: task.bundle.bundle_name.clone(),
                runtime_directory: task.runtime_directory.clone(),
                target_member: target_member.clone(),
                choose: noop_tmux_chooser(),
            };
            // Propagated rather than discarded. A dropped startup error installed
            // an already-dead transport, and the health gate then held the
            // triggering member through the whole dwell before reporting a generic
            // unreachable -- turning a spawn failure the relay already knew about
            // into a delayed guess.
            start_transport_off_runtime(transport, context, task.target_session.clone()).await
        }
        #[cfg(not(feature = "pty"))]
        SessionType::Pty => Err(relay_error(
            "internal_unexpected_failure",
            "PTY target configured but pty Cargo feature is disabled",
            Some(json!({ "target_session": task.target_session })),
        )),
        SessionType::Acp => Err(relay_error(
            "internal_unexpected_failure",
            "ACP target reached the non-bootstrap worker construction path",
            Some(json!({ "target_session": task.target_session })),
        )),
    }
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
/// for tmux sessions (`[coders.<id>.tmux].prime-timeout-ms`),
/// `AcpTargetConfiguration.prime_timeout_ms` for ACP sessions
/// (`[coders.<id>.acp].prime-timeout-ms`), and the Pty equivalent. Each
/// transport consumes the same generic envelope field; no transport-prefixed
/// envelope field is introduced.
///
/// [`DeliveryEnvelope::readiness_timeout_ms`] comes from
/// [`TargetConfiguration::readiness_timeout_ms`], which is the sole place the
/// per-transport rule is decided: **Tmux targets only**, from
/// `[coders.<id>.tmux].readiness-timeout-ms`. Every other target gets `None` —
/// not because their waits are bounded elsewhere, but because only Tmux can
/// soundly report an expired bound as non-delivery (it injects after its
/// readiness wait; Pty and ACP commit before theirs). See
/// `agentmux:issues/relay/61`.
///
/// For terminal-outcome receipts addressed to an ACP sender the envelope's
/// `quiet_window` is forced to zero, satisfying the
/// "receipt-bypasses-quiescence" invariant on ACP. ACP ignores `quiet_window`
/// today, but the construction is explicit at the relay so the invariant
/// holds regardless of whether the ACP transport starts honoring it in a
/// future change. Receipts addressed to Tmux/Pty senders keep the default
/// async quiet-window so the per-transport quiescence behavior is unchanged
/// for those senders.
pub(super) fn build_coder_envelope(
    task: &AsyncDeliveryTask,
    message: DeliveryMessage,
) -> DeliveryEnvelope {
    let (prime_timeout_ms, readiness_timeout_ms, target_is_acp) = match resolve_target_member(task)
    {
        Ok(Some(member)) => {
            let readiness_timeout_ms = member.target.readiness_timeout_ms();
            match &member.target {
                TargetConfiguration::Tmux(tmux_target) => {
                    (tmux_target.prime_timeout_ms, readiness_timeout_ms, false)
                }
                TargetConfiguration::Acp(acp_target) => {
                    (acp_target.prime_timeout_ms, readiness_timeout_ms, true)
                }
                TargetConfiguration::Pty(pty_target) => {
                    (pty_target.prime_timeout_ms, readiness_timeout_ms, false)
                }
                TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
                    (None, readiness_timeout_ms, false)
                }
            }
        }
        _ => (None, None, false),
    };
    let quiet_window = if task.is_receipt && target_is_acp {
        Duration::ZERO
    } else {
        task.quiescence.quiet_window
    };
    DeliveryEnvelope {
        message_id: task.message_id.clone(),
        message,
        append_enter: true,
        choice_decider_sessions: task.choice_decider_sessions.clone(),
        quiet_window,
        prime_timeout_ms,
        readiness_timeout_ms,
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
/// rendering pane text. The UI transport owns its own reconnect cap
/// ([`UI_RECONNECT_TIMEOUT_MS_DEFAULT`](crate::transports::ui::UI_RECONNECT_TIMEOUT_MS_DEFAULT));
/// the relay no longer threads an external knob through the envelope.
pub(super) fn build_ui_envelope(task: &AsyncDeliveryTask) -> DeliveryEnvelope {
    let target_member = task
        .bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session);
    let message = build_delivery_message(task, target_member, outcomes::now_rfc3339().as_str());
    DeliveryEnvelope {
        message_id: task.message_id.clone(),
        message,
        append_enter: task.append_enter,
        choice_decider_sessions: task.choice_decider_sessions.clone(),
        quiet_window: task.quiescence.quiet_window,
        prime_timeout_ms: None,
        readiness_timeout_ms: None,
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

/// Inline because [`build_coder_envelope`] is `pub(super)` and takes
/// [`AsyncDeliveryTask`], a relay-internal type. Reaching it from
/// `tests/unit` would mean widening both to `pub` — publishing the relay's
/// dispatch internals as API to observe one field assignment. The
/// per-transport *rule* is covered publicly through
/// `TargetConfiguration::readiness_timeout_ms`; what only this seam can show is
/// that the builder puts the resolved value on the envelope it emits.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::configuration::{
        AcpChannel, AcpTargetConfiguration, BundleConfiguration, BundleMember,
        PtyTargetConfiguration, TermProtocol, TmuxTargetConfiguration,
    };
    use crate::envelope::AddressIdentity;
    use crate::transports::DeliveryPayloadMode;

    const SCHEMA_VERSION: &str = "1";
    const TEST_BOUND_MS: u64 = 45_000;

    fn member(target: TargetConfiguration) -> BundleMember {
        BundleMember {
            id: "target".to_string(),
            name: None,
            working_directory: None,
            target,
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }

    fn task_for(target: TargetConfiguration) -> AsyncDeliveryTask {
        let target_member = member(target);
        AsyncDeliveryTask {
            // Test fixture: constructed directly, never admitted.
            admitted: false,
            bundle: BundleConfiguration {
                schema_version: SCHEMA_VERSION.to_string(),
                bundle_name: "party".to_string(),
                autostart: false,
                groups: Vec::new(),
                members: vec![target_member.clone()],
            },
            sender_namespace: "party".to_string(),
            sender: target_member,
            authenticated_identity: None,
            on_behalf_of: None,
            all_target_sessions: Vec::new(),
            target_session: "target".to_string(),
            message: "body".to_string(),
            message_id: "message-id".to_string(),
            quiescence: crate::relay::delivery::QuiescenceOptions::for_async(None),
            runtime_directory: PathBuf::from("/runtime"),
            payload_mode: DeliveryPayloadMode::EnvelopeMessage,
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: false,
            sender_return_route: None,
        }
    }

    fn delivery_message() -> DeliveryMessage {
        DeliveryMessage {
            body: "body".to_string(),
            created_at: "2026-08-01T00:00:00Z".to_string(),
            namespace: "party".to_string(),
            sender: AddressIdentity {
                session_name: "sender".to_string(),
                display_name: None,
            },
            target: AddressIdentity {
                session_name: "target".to_string(),
                display_name: None,
            },
            cc: Vec::new(),
            authenticated_identity: None,
            on_behalf_of: None,
        }
    }

    /// Task 4.11 — the builder places the resolved readiness bound on the
    /// envelope for Tmux and leaves every other transport's at `None`.
    ///
    /// Table-driven over all five target variants: a bound that leaked to Pty
    /// or ACP would make those transports report an expired wait as
    /// non-delivery, which they cannot do soundly — they commit the message
    /// before their readiness wait, so the target may already hold it.
    #[test]
    fn build_coder_envelope_carries_the_readiness_bound_for_tmux_only() {
        let cases = [
            (
                TargetConfiguration::Tmux(TmuxTargetConfiguration {
                    start_command: "run".to_string(),
                    prompt_readiness: None,
                    prime_timeout_ms: None,
                    readiness_timeout_ms: TEST_BOUND_MS,
                }),
                Some(TEST_BOUND_MS),
            ),
            (
                TargetConfiguration::Pty(PtyTargetConfiguration {
                    initial_command: "/bin/cat".to_string(),
                    resume_command: "/bin/cat".to_string(),
                    prompt_readiness: None,
                    prime_timeout_ms: None,
                    wedge_detection: true,
                    cols: 120,
                    rows: 40,
                    term_protocol: TermProtocol::default(),
                }),
                None,
            ),
            (
                TargetConfiguration::Acp(AcpTargetConfiguration {
                    channel: AcpChannel::Stdio,
                    command: Some("acp-shell".to_string()),
                    url: None,
                    prime_timeout_ms: None,
                    headers: Vec::new(),
                }),
                None,
            ),
            (TargetConfiguration::Ui, None),
            (TargetConfiguration::Pubsub, None),
        ];
        for (target, expected) in cases {
            let label = target.session_type();
            let envelope = build_coder_envelope(&task_for(target), delivery_message());
            assert_eq!(
                envelope.readiness_timeout_ms, expected,
                "wrong readiness bound on the envelope for {label:?}",
            );
        }
    }
}
