//! ACP delivery as a [`Transport`] implementation.
//!
//! `AcpTransport` owns the per-target `PersistentAcpWorkerRuntime` (moved here
//! from the relay delivery worker, which previously threaded it through
//! `spawn_blocking`). [`Transport::deliver`] submits one ACP prompt and BLOCKS
//! until the turn reaches a terminal state, folding in what used to be the
//! reader thread's `on_completion` body; the relay worker fans the single
//! terminal outcome out to the coalesced tasks.
//!
//! Choices (tool-call permissions) resolve through the relay-injected
//! [`Chooser`] (see [`crate::acp::permission`]); the transport never calls the
//! relay choice queue directly. The `look` path reads output through the
//! [`OutputView`] handle published by [`Transport::give_output`].
//!
//! ## Readiness
//!
//! The transport owns an [`AcpWorkerReadinessState`] signal for [`is_ready`] and
//! the [`OutputView`] prime-wait, because it cannot call relay's
//! `set_acp_worker_state`. The relay worker mirrors transitions into the global
//! worker-state registry (which external observers and respawn/startup gating
//! still read).
//!
//! [`is_ready`]: Transport::is_ready

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::acp::permission::{ChoiceCorrelation, build_acp_permission_handler};
use crate::acp::state::{
    AcpLookSnapshot, derive_acp_look_snapshot, load_persisted_acp_session_id,
    persist_acp_session_id,
};
use crate::acp::{
    AcpStdioClient, DispatchHandler, PromptCompletion, PromptCompletionHandler,
    PromptDispatchOutcome, ReplayEntry,
};
use crate::configuration::{AcpChannel, AcpTargetConfiguration, BundleMember, TargetConfiguration};
use crate::relay::{AcpWorkerReadinessState, SendOutcome};
use crate::runtime::signals::shutdown_requested;
use crate::transports::{
    ChoiceMade, DeliveryContext, DeliveryEnvelope, DeliveryResult, LookMode, LookSnapshotPayload,
    OutputView, RawWriteResult, SingleDeliveryOutcome, StartupContext, Transport, TransportError,
    TransportReadiness, TransportStatus,
};

// ACP delivery failure taxonomy (see the relay delivery README for the full
// catalogue). These mirror the codes the relay completion path used before the
// transport move so the wire outcomes are unchanged.
const ACP_REASON_CODE_STOP_CANCELLED: &str = "acp_stop_cancelled";
/// Bootstrap initialize failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_INITIALIZE_FAILED: &str = "runtime_acp_initialize_failed";
const ACP_ERROR_CODE_SESSION_LOAD_FAILED: &str = "runtime_acp_session_load_failed";
const ACP_ERROR_CODE_SESSION_NEW_FAILED: &str = "runtime_acp_session_new_failed";
/// Prompt-dispatch failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_PROMPT_FAILED: &str = "runtime_acp_prompt_failed";
/// Connection-closed failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_CONNECTION_CLOSED: &str = "runtime_acp_connection_closed";
/// Transport-unavailable failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE: &str = "acp_child_unavailable";
const ACP_ERROR_CODE_MISSING_CAPABILITY: &str = "validation_missing_acp_capability";
const ACP_ERROR_CODE_WORKER_UNAVAILABLE: &str = "runtime_acp_worker_unavailable";

const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";

/// Slice length for the single-flight ACP prompt-completion wait. Bounds how
/// long the blocking thread parks before re-checking the shutdown gate.
const ACP_PROMPT_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Poll cadence for the look prime-wait.
const ACP_LOOK_PRIME_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Fallback tail-window size when the relay does not resolve a concrete value.
const ACP_LOOK_ENTRIES_FALLBACK: usize = 64;

/// The persistent ACP runtime owned by an [`AcpTransport`]: the stdio client and
/// the resolved session id used for every prompt.
pub struct PersistentAcpWorkerRuntime {
    pub client: AcpStdioClient,
    pub session_id: String,
}

/// A structured bootstrap failure. Surfaced to the relay worker so it can decide
/// whether respawn might recover or the failure is permanent.
#[derive(Clone, Debug)]
pub struct AcpBootstrapError {
    pub code: String,
    pub reason: String,
}

impl AcpBootstrapError {
    /// Permanent failures are conditions respawn cannot resolve: a capability gap
    /// means the agent fundamentally cannot host the session, so retrying with the
    /// same binary reproduces the failure.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        self.code == ACP_ERROR_CODE_MISSING_CAPABILITY
    }
}

#[derive(Clone, Copy, Debug)]
enum AcpLifecycleSelection {
    NewSession,
    LoadSession,
}

#[derive(Clone, Debug)]
struct AcpCapabilities {
    load_session: bool,
    prompt_session: bool,
}

/// ACP delivery transport. Owns the runtime, the injected [`Chooser`], and the
/// shared readiness signal the [`OutputView`] reads.
pub struct AcpTransport {
    runtime: Option<PersistentAcpWorkerRuntime>,
    chooser: Option<crate::transports::Chooser>,
    readiness: Arc<Mutex<AcpWorkerReadinessState>>,
}

impl Default for AcpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AcpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpTransport")
            .field("has_runtime", &self.runtime.is_some())
            .field(
                "readiness",
                &*self.readiness.lock().expect("readiness mutex"),
            )
            .finish()
    }
}

impl AcpTransport {
    #[must_use]
    pub fn new() -> Self {
        Self {
            runtime: None,
            chooser: None,
            readiness: Arc::new(Mutex::new(AcpWorkerReadinessState::Initializing)),
        }
    }

    /// Current readiness, mirrored by the relay worker into the global registry.
    #[must_use]
    pub fn readiness(&self) -> AcpWorkerReadinessState {
        *self.readiness.lock().expect("readiness mutex")
    }

    fn set_readiness(&self, state: AcpWorkerReadinessState) {
        *self.readiness.lock().expect("readiness mutex") = state;
    }
}

impl Transport for AcpTransport {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        self.chooser = Some(context.choose);
        self.set_readiness(AcpWorkerReadinessState::Initializing);
        match bootstrap_acp_worker_runtime(&context.runtime_directory, &context.target_member) {
            Ok(runtime) => {
                self.runtime = Some(runtime);
                self.set_readiness(AcpWorkerReadinessState::Available);
                Ok(TransportStatus {
                    readiness: TransportReadiness::Ready,
                })
            }
            Err(error) => {
                self.runtime = None;
                self.set_readiness(AcpWorkerReadinessState::Unavailable);
                Err(TransportError {
                    code: error.code,
                    reason: error.reason,
                    details: None,
                })
            }
        }
    }

    fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult {
        // ACP coalescing happens relay-side: the worker pre-combines the batch
        // into a single rendered prompt and replicates the returned outcome
        // across the coalesced tasks. Exactly one envelope is expected here.
        let Some(envelope) = envelopes.into_iter().next() else {
            return single(failed_outcome(
                context.target_session.clone(),
                String::new(),
                "ACP delivery received no envelope",
            ));
        };
        let target_session = context.target_session.clone();
        let message_id = envelope.message_id.clone();
        let target_member_id = context.target_member.id.clone();

        if context.target_member.working_directory.is_none() {
            return single(failed_outcome(
                target_session,
                message_id,
                "ACP target is missing working directory",
            ));
        }
        if matches!(self.readiness(), AcpWorkerReadinessState::Unavailable) {
            return single(worker_unavailable_outcome(
                target_session,
                message_id,
                target_member_id.as_str(),
            ));
        }
        let Some(chooser) = self.chooser.clone() else {
            return single(worker_unavailable_outcome(
                target_session,
                message_id,
                target_member_id.as_str(),
            ));
        };
        let readiness = Arc::clone(&self.readiness);
        let Some(runtime) = self.runtime.as_mut() else {
            return single(worker_unavailable_outcome(
                target_session,
                message_id,
                target_member_id.as_str(),
            ));
        };

        let pending_choice: Arc<Mutex<Option<ChoiceMade>>> = Arc::new(Mutex::new(None));
        let completion_slot: Arc<Mutex<Option<PromptCompletion>>> = Arc::new(Mutex::new(None));

        let readiness_for_dispatch = Arc::clone(&readiness);
        let on_dispatched: DispatchHandler = Box::new(move || {
            *readiness_for_dispatch.lock().expect("readiness mutex") =
                AcpWorkerReadinessState::Busy;
        });

        let correlation = ChoiceCorrelation {
            message_id: message_id.clone(),
            target_session: target_session.clone(),
            pending_max: context.choices_pending_max,
            decider_sessions: context.choice_decider_sessions.clone(),
        };
        let on_permission =
            build_acp_permission_handler(chooser, correlation, Arc::clone(&pending_choice));

        let completion_writer = Arc::clone(&completion_slot);
        let on_completion: PromptCompletionHandler = Box::new(move |completion| {
            *completion_writer.lock().expect("completion slot mutex") = Some(completion);
        });

        let session_id = runtime.session_id.clone();
        let dispatch = runtime.client.prompt(
            session_id.as_str(),
            envelope.rendered.as_str(),
            Some(on_dispatched),
            Some(on_permission),
            on_completion,
        );

        match dispatch {
            PromptDispatchOutcome::Submitted => {
                // Block until the turn reaches a terminal state. Polled in bounded
                // slices so process shutdown can abandon a never-completing turn.
                loop {
                    if runtime
                        .client
                        .wait_for_prompt_complete(ACP_PROMPT_WAIT_POLL_INTERVAL)
                    {
                        break;
                    }
                    if shutdown_requested() {
                        break;
                    }
                }
                let completion = completion_slot
                    .lock()
                    .expect("completion slot mutex")
                    .take();
                let pending = pending_choice.lock().expect("pending_choice mutex").take();
                let (final_state, outcome) = build_acp_completion_result(
                    completion,
                    pending,
                    target_session,
                    message_id,
                    target_member_id.as_str(),
                );
                self.set_readiness(final_state);
                single(outcome)
            }
            PromptDispatchOutcome::TransportUnavailable { reason } => {
                self.set_readiness(AcpWorkerReadinessState::Unavailable);
                single(failed_outcome_with_code(
                    target_session,
                    message_id,
                    ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
                    "ACP child stdin write failed",
                    Some(json!({ "target_session": target_member_id, "reason": reason })),
                ))
            }
            PromptDispatchOutcome::SerializationFailed(reason) => {
                self.set_readiness(AcpWorkerReadinessState::Unavailable);
                single(failed_outcome_with_code(
                    target_session,
                    message_id,
                    ACP_ERROR_CODE_PROMPT_FAILED,
                    "ACP session/prompt dispatch failed",
                    Some(json!({ "target_session": target_member_id, "reason": reason })),
                ))
            }
        }
    }

    fn is_ready(&self) -> bool {
        matches!(
            self.readiness(),
            AcpWorkerReadinessState::Available | AcpWorkerReadinessState::Busy
        )
    }

    fn raw_write(
        &mut self,
        text: &str,
        _append_enter: bool,
        context: &DeliveryContext,
    ) -> RawWriteResult {
        // ACP raww submits the raw text as a prompt and blocks to terminal, the
        // same submit path envelope delivery uses. The relay routes RawInput
        // tasks through `deliver()` (rendered = raw text), so this method exists
        // for contract completeness and direct raww callers.
        let envelope = DeliveryEnvelope {
            message_id: String::new(),
            payload_mode: crate::relay::DeliveryPayloadMode::RawInput,
            rendered: text.to_string(),
            append_enter: _append_enter,
        };
        let result = self.deliver(vec![envelope], context);
        match result.outcomes.into_iter().next() {
            Some(outcome) if matches!(outcome.outcome, SendOutcome::Delivered) => {
                RawWriteResult::Written
            }
            Some(outcome) => RawWriteResult::Failed {
                reason: outcome
                    .reason
                    .unwrap_or_else(|| "ACP raw write failed".to_string()),
            },
            None => RawWriteResult::Failed {
                reason: "ACP raw write produced no outcome".to_string(),
            },
        }
    }

    fn shutdown(&mut self) {
        // Dropping the runtime joins the child and reader thread (its `Drop`
        // kills the child).
        self.runtime = None;
        self.set_readiness(AcpWorkerReadinessState::Unavailable);
    }

    fn accept_capacity(&self) -> usize {
        // ACP accepts the full batch; the relay peels by token budget upstream.
        usize::MAX
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        let runtime = self.runtime.as_ref()?;
        Some(Arc::new(AcpOutputView {
            replay: runtime.client.replay_buffer_handle(),
            readiness: Arc::clone(&self.readiness),
        }))
    }
}

/// Concurrent look view over an ACP transport's replay buffer. Captures the
/// shared replay buffer and readiness signal so the relay look path can read a
/// snapshot without borrowing the worker-owned transport.
struct AcpOutputView {
    replay: Arc<Mutex<Vec<ReplayEntry>>>,
    readiness: Arc<Mutex<AcpWorkerReadinessState>>,
}

impl OutputView for AcpOutputView {
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        // Own the bounded prime-wait: while the worker is still initializing,
        // wait up to `prime_timeout` for the first snapshot to populate.
        let deadline = Instant::now() + mode.prime_timeout;
        let prime_timed_out = loop {
            let state = *self.readiness.lock().expect("readiness mutex");
            if !matches!(state, AcpWorkerReadinessState::Initializing) {
                break false;
            }
            if Instant::now() >= deadline {
                break true;
            }
            thread::sleep(ACP_LOOK_PRIME_POLL_INTERVAL);
        };

        let worker_state = *self.readiness.lock().expect("readiness mutex");
        let entries = self.replay.lock().expect("replay buffer mutex").clone();
        let requested_entries = mode
            .lines
            .map(|lines| lines as usize)
            .unwrap_or(ACP_LOOK_ENTRIES_FALLBACK);
        let offset = mode.offset.map(|offset| offset as usize).unwrap_or(0);
        let snapshot = derive_acp_look_snapshot(
            Some(worker_state),
            Some(entries.as_slice()),
            requested_entries,
            offset,
            prime_timed_out,
        );
        Ok(acp_snapshot_to_payload(snapshot))
    }
}

fn acp_snapshot_to_payload(snapshot: AcpLookSnapshot) -> LookSnapshotPayload {
    LookSnapshotPayload::AcpEntries {
        snapshot_entries: snapshot.snapshot_entries,
        entries_total: snapshot.entries_total,
        returned_entries_count: snapshot.returned_entries_count,
        freshness: snapshot.freshness,
        snapshot_source: snapshot.snapshot_source,
        stale_reason_code: snapshot.stale_reason_code,
        snapshot_age_ms: snapshot.snapshot_age_ms,
    }
}

/// Builds the per-target ACP runtime. Used by the relay worker for initial
/// bootstrap and respawn (the worker re-publishes the [`OutputView`] handle
/// afterward via [`Transport::give_output`]).
pub fn bootstrap_acp_worker_runtime(
    runtime_directory: &Path,
    target_member: &BundleMember,
) -> Result<PersistentAcpWorkerRuntime, AcpBootstrapError> {
    let TargetConfiguration::Acp(acp_target) = &target_member.target else {
        return Err(AcpBootstrapError {
            code: "runtime_startup_failed".to_string(),
            reason: "ACP worker bootstrap requires ACP target".to_string(),
        });
    };
    let Some(working_directory) = target_member.working_directory.as_ref() else {
        return Err(AcpBootstrapError {
            code: "runtime_startup_failed".to_string(),
            reason: "ACP worker bootstrap requires target working directory".to_string(),
        });
    };
    initialize_persistent_acp_worker_runtime(
        target_member,
        acp_target,
        working_directory.as_path(),
        runtime_directory,
    )
}

fn single(outcome: SingleDeliveryOutcome) -> DeliveryResult {
    DeliveryResult {
        outcomes: vec![outcome],
    }
}

fn delivered_outcome(target_session: String, message_id: String) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

fn failed_outcome(
    target_session: String,
    message_id: String,
    reason: impl Into<String>,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::Failed,
        reason_code: None,
        reason: Some(reason.into()),
        details: None,
    }
}

fn failed_outcome_with_code(
    target_session: String,
    message_id: String,
    reason_code: &str,
    reason: impl Into<String>,
    details: Option<Value>,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::Failed,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.into()),
        details,
    }
}

fn worker_unavailable_outcome(
    target_session: String,
    message_id: String,
    target_member_id: &str,
) -> SingleDeliveryOutcome {
    failed_outcome_with_code(
        target_session,
        message_id,
        ACP_ERROR_CODE_WORKER_UNAVAILABLE,
        "ACP worker is unavailable for target session",
        Some(json!({ "target_session": target_member_id })),
    )
}

fn build_acp_completion_result(
    completion: Option<PromptCompletion>,
    pending_choice_outcome: Option<ChoiceMade>,
    target_session: String,
    message_id: String,
    target_member_id: &str,
) -> (AcpWorkerReadinessState, SingleDeliveryOutcome) {
    if let Some(ChoiceMade::Cancelled {
        reason_code,
        reason,
        ..
    }) = pending_choice_outcome
    {
        return (
            AcpWorkerReadinessState::Available,
            failed_outcome_with_code(
                target_session,
                message_id,
                reason_code.as_str(),
                reason.unwrap_or_else(|| "choice request was cancelled".to_string()),
                Some(json!({ "target_session": target_member_id })),
            ),
        );
    }

    let Some(completion) = completion else {
        // No completion observed before the wait was abandoned: shutdown.
        return (
            AcpWorkerReadinessState::Available,
            failed_outcome_with_code(
                target_session,
                message_id,
                DROPPED_ON_SHUTDOWN_REASON_CODE,
                DROPPED_ON_SHUTDOWN_REASON,
                None,
            ),
        );
    };

    match completion {
        PromptCompletion::Completed { stop_reason } => match stop_reason.as_str() {
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => (
                AcpWorkerReadinessState::Available,
                delivered_outcome(target_session, message_id),
            ),
            "cancelled" => (
                AcpWorkerReadinessState::Available,
                failed_outcome_with_code(
                    target_session,
                    message_id,
                    ACP_REASON_CODE_STOP_CANCELLED,
                    "ACP turn completed with stopReason=cancelled",
                    None,
                ),
            ),
            other => (
                AcpWorkerReadinessState::Available,
                failed_outcome(
                    target_session,
                    message_id,
                    format!("ACP returned unsupported stopReason '{other}'"),
                ),
            ),
        },
        PromptCompletion::ProtocolError(reason) => (
            AcpWorkerReadinessState::Available,
            failed_outcome_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt failed",
                Some(json!({ "target_session": target_member_id, "reason": reason })),
            ),
        ),
        PromptCompletion::ConnectionClosed { reason } => (
            AcpWorkerReadinessState::Unavailable,
            failed_outcome_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_CONNECTION_CLOSED,
                "ACP connection closed before prompt response",
                Some(json!({ "target_session": target_member_id, "reason": reason })),
            ),
        ),
    }
}

fn initialize_persistent_acp_worker_runtime(
    target_member: &BundleMember,
    acp: &AcpTargetConfiguration,
    working_directory: &Path,
    runtime_directory: &Path,
) -> Result<PersistentAcpWorkerRuntime, AcpBootstrapError> {
    let mut client = match acp.channel {
        AcpChannel::Stdio => {
            let Some(command) = acp.command.as_deref() else {
                return Err(AcpBootstrapError {
                    code: "runtime_startup_failed".to_string(),
                    reason: "ACP stdio target requires command".to_string(),
                });
            };
            AcpStdioClient::spawn(
                command,
                working_directory,
                &acp.environment
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.value.clone()))
                    .collect::<Vec<_>>(),
            )
            .map_err(|reason| AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason,
            })?
        }
        AcpChannel::Http => {
            return Err(AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason: "ACP http transport is not implemented".to_string(),
            });
        }
    };

    let initialize_result = client.initialize().map_err(|reason| AcpBootstrapError {
        code: ACP_ERROR_CODE_INITIALIZE_FAILED.to_string(),
        reason: format!("ACP initialize failed: {reason}"),
    })?;

    let capabilities = AcpCapabilities {
        load_session: initialize_result
            .get("agentCapabilities")
            .and_then(|value| value.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        prompt_session: initialize_result
            .get("agentCapabilities")
            .map(|value| {
                value
                    .get("promptSession")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| {
                        value
                            .get("promptCapabilities")
                            .is_some_and(serde_json::Value::is_object)
                    })
            })
            .unwrap_or(false),
    };

    let persisted_session_id = if target_member.coder_session_id.is_some() {
        None
    } else {
        load_persisted_acp_session_id(runtime_directory, target_member.id.as_str()).map_err(
            |reason| AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason: format!("failed to load persisted ACP session id: {reason}"),
            },
        )?
    };

    let (lifecycle, lifecycle_session_id) =
        if let Some(configured) = target_member.coder_session_id.as_deref() {
            (AcpLifecycleSelection::LoadSession, configured.to_string())
        } else if let Some(persisted) = persisted_session_id {
            (AcpLifecycleSelection::LoadSession, persisted)
        } else {
            (AcpLifecycleSelection::NewSession, String::new())
        };

    let session_id = match lifecycle {
        AcpLifecycleSelection::LoadSession => {
            if !capabilities.load_session {
                return Err(AcpBootstrapError {
                    code: ACP_ERROR_CODE_MISSING_CAPABILITY.to_string(),
                    reason: "ACP agent does not advertise required load capability".to_string(),
                });
            }
            client
                .load_session(lifecycle_session_id.as_str(), working_directory)
                .map_err(|reason| AcpBootstrapError {
                    code: ACP_ERROR_CODE_SESSION_LOAD_FAILED.to_string(),
                    reason: format!("ACP session/load failed: {reason}"),
                })?;
            lifecycle_session_id
        }
        AcpLifecycleSelection::NewSession => {
            client
                .new_session(working_directory)
                .map_err(|reason| AcpBootstrapError {
                    code: ACP_ERROR_CODE_SESSION_NEW_FAILED.to_string(),
                    reason: format!("ACP session/new failed: {reason}"),
                })?
        }
    };

    persist_acp_session_id(
        runtime_directory,
        target_member.id.as_str(),
        session_id.as_str(),
    )
    .map_err(|reason| AcpBootstrapError {
        code: "runtime_startup_failed".to_string(),
        reason: format!("failed to persist ACP session id: {reason}"),
    })?;

    if !capabilities.prompt_session {
        return Err(AcpBootstrapError {
            code: ACP_ERROR_CODE_MISSING_CAPABILITY.to_string(),
            reason: "ACP agent does not advertise required prompt capability".to_string(),
        });
    }

    Ok(PersistentAcpWorkerRuntime { client, session_id })
}
