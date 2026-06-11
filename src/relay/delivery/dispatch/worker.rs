use std::{
    collections::VecDeque,
    sync::OnceLock,
    time::{Duration, Instant},
};

use serde_json::json;
use time::format_description::well_known::Rfc3339;
use tokio::{runtime::Handle, sync::mpsc::UnboundedReceiver};

use crate::{
    configuration::{BundleMember, TargetConfiguration, TmuxTargetConfiguration},
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
};

use super::super::super::canonical_session_id;
use super::super::super::stream::{
    RelayStreamEvent, broadcast_event_to_bundle_ui, list_registered_ui_sessions_for_bundle,
};
use super::super::super::{AsyncDeliveryTask, DeliveryPayloadMode, RelayError, SendResult};
use super::super::acp_delivery::{
    ACP_ERROR_CODE_CONNECTION_CLOSED, ACP_ERROR_CODE_INITIALIZE_FAILED,
    ACP_ERROR_CODE_PROMPT_FAILED, ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE, AcpBootstrapError,
    PersistentAcpWorkerRuntime, bootstrap_acp_worker_runtime, respawn_acp_worker_runtime,
};
use super::super::async_worker::{
    AcpWorkerReadinessState, AsyncWorkerKey, get_acp_worker_state,
    install_acp_worker_replay_buffer, set_acp_worker_state,
};
use super::super::permission_state::{PermissionEventContext, invalidate_pending_for_respawn};

const RESPAWN_BACKOFF_MAX_MS_ENVVAR: &str = "AGENTMUX_RELAY_ACP_RESPAWN_BACKOFF_MAX_MS";
const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;
/// Slice length for the single-flight ACP prompt-completion wait. Bounds how
/// long the worker's blocking thread parks before re-checking the shutdown gate
/// so a never-completing agent turn cannot pin teardown.
const ACP_PROMPT_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const RESPAWN_SLEEP_POLL_MS: u64 = 50;
const RESPAWN_BACKOFF_INITIAL_MS: u64 = 1_000;
const RESPAWN_BACKOFF_CAP_DEFAULT_MS: u64 = 30_000;
const RESPAWN_INIT_FAILURE_THRESHOLD: u32 = 3;
const BATCH_DRAIN_MAX_ENVVAR: &str = "AGENTMUX_RELAY_BATCH_DRAIN_MAX";
const BATCH_DRAIN_MAX_DEFAULT: usize = 32;

#[derive(Clone)]
pub(super) struct AcpWorkerBootstrap {
    pub(super) target_member: BundleMember,
    pub(super) runtime_directory: std::path::PathBuf,
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
    let acp_context = bootstrap.clone();
    let mut acp_runtime = if let Some(bootstrap) = bootstrap {
        bootstrap_acp_runtime_on_worker_start(&key, bootstrap).await
    } else {
        None
    };
    let mut respawn_state = AcpRespawnState::new();
    let poll_interval = Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS);
    let drain_max = batch_drain_max();
    // Carry buffer: tasks consumed from the channel that did not coalesce with
    // the current head (different payload mode or UI-routing), plus ACP tasks
    // peeled back because the rendered prompt exceeded one budget batch. Drained
    // before the channel on the next iteration so original ordering survives.
    let mut carry: VecDeque<AsyncDeliveryTask> = VecDeque::new();

    loop {
        if shutdown_requested() {
            drain_carry_on_shutdown(&mut carry, pending.as_ref());
            super::super::async_worker::drop_pending_async_tasks_on_shutdown(
                &mut receiver,
                pending.as_ref(),
            );
            break;
        }
        let head = if let Some(carried) = carry.pop_front() {
            carried
        } else {
            let received = tokio::select! {
                biased;
                value = receiver.recv() => value,
                _ = tokio::time::sleep(poll_interval) => {
                    // Poll-tick: re-evaluate shutdown gate without consuming a task.
                    continue;
                }
            };
            match received {
                Some(task) => task,
                // All senders dropped; worker is no longer reachable.
                None => break,
            }
        };
        if shutdown_requested() {
            super::super::async_worker::complete_task_on_shutdown(&head);
            super::super::async_worker::release_pending_slot(pending.as_ref());
            drain_carry_on_shutdown(&mut carry, pending.as_ref());
            super::super::async_worker::drop_pending_async_tasks_on_shutdown(
                &mut receiver,
                pending.as_ref(),
            );
            break;
        }

        let mut batch = coalesce_batch(head, drain_max, &mut carry, &mut receiver);
        let pre_quiescence_count = batch.len();

        // Tmux envelope-mode heads: hoist the quiescence wait out of the
        // transport so any tasks arriving during the wait can be coalesced
        // into this batch via a post-quiescence try_recv drain. ACP/UI
        // targets and RawInput heads pass through with `pre_resolved_pane`
        // unset; their transport paths are unchanged.
        let pre_resolved_pane = match classify_tmux_quiescence_hoist(&batch[0]) {
            Some(tmux_target) => {
                let head_task = batch[0].clone();
                let wait_outcome = tokio::task::spawn_blocking(move || {
                    super::transport::prepare_tmux_pane_for_envelope_head(&head_task, &tmux_target)
                })
                .await
                .expect("tmux quiescence hoist task panicked");
                match wait_outcome {
                    Ok(pane_target) => {
                        extend_batch_with_drain(&mut batch, drain_max, &mut carry, &mut receiver);
                        Some(pane_target)
                    }
                    Err(boxed_template) => {
                        complete_batch_with_template(&batch, *boxed_template, pending.as_ref());
                        continue;
                    }
                }
            }
            None => None,
        };
        let post_quiescence_count = batch.len() - pre_quiescence_count;

        if batch.len() > 1 {
            emit_inscription(
                "relay.send.batch_drain.coalesced",
                &json!({
                    "bundle_name": batch[0].bundle.bundle_name,
                    "target_session": batch[0].target_session,
                    "drained_count": batch.len(),
                    "pre_quiescence_count": pre_quiescence_count,
                    "post_quiescence_count": post_quiescence_count,
                    "message_ids": batch.iter().map(|task| &task.message_id).collect::<Vec<_>>(),
                }),
            );
        }
        let (outcomes, returned_runtime, deferred) =
            deliver_batch_blocking(batch.clone(), pre_resolved_pane, acp_runtime).await;
        acp_runtime = returned_runtime;
        // Push deferred (ACP-peeled) tasks back to the front of the carry queue
        // in original order so they are the head of the next iteration.
        for deferred_task in deferred.into_iter().rev() {
            carry.push_front(deferred_task);
        }
        let trigger_reason = outcomes
            .first()
            .map(classify_respawn_trigger)
            .unwrap_or("worker_unavailable");
        for (task, outcome) in batch.iter().zip(outcomes) {
            super::super::async_worker::complete_task_outcome(task, outcome);
            super::super::async_worker::release_pending_slot(pending.as_ref());
        }

        // Per-target ACP single-flight: block until the previous prompt is
        // fully complete (background reader fired `on_completion`, or
        // synchronous dispatch failure already cleared the slot) before
        // pulling the next task. `wait_for_prompt_complete()` is a blocking
        // mpsc recv inside `AcpStdioClient`; run it on the blocking pool so
        // the tokio worker thread is not pinned.
        if acp_runtime.is_some() {
            acp_runtime = wait_for_prompt_complete_blocking(acp_runtime).await;
        }

        if let Some(ctx) = acp_context.as_ref() {
            let state = get_acp_worker_state(
                key.bundle_name.as_str(),
                ctx.runtime_directory.as_path(),
                ctx.target_member.id.as_str(),
            );
            if matches!(state, Some(AcpWorkerReadinessState::Unavailable)) {
                drive_acp_worker_respawn(
                    &key,
                    ctx,
                    trigger_reason,
                    &mut respawn_state,
                    &mut acp_runtime,
                )
                .await;
            } else if matches!(
                state,
                Some(AcpWorkerReadinessState::Available | AcpWorkerReadinessState::Busy)
            ) {
                respawn_state.reset_on_success();
            }
        }
    }
    super::super::async_worker::unregister_worker(&key);
}

fn batch_drain_max() -> usize {
    std::env::var(BATCH_DRAIN_MAX_ENVVAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(BATCH_DRAIN_MAX_DEFAULT)
}

/// Collects up to `drain_max` tasks into a coalesce batch starting with `head`.
///
/// Only `EnvelopeMessage` tasks targeting non-UI sessions coalesce. The first
/// task whose payload mode or UI-routing differs from the head is pushed to
/// the front of `carry` so it heads the next worker iteration. The channel
/// is read non-blocking via `try_recv`; an empty channel ends the drain.
pub(super) fn coalesce_batch(
    head: AsyncDeliveryTask,
    drain_max: usize,
    carry: &mut VecDeque<AsyncDeliveryTask>,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
) -> Vec<AsyncDeliveryTask> {
    let coalescable = matches!(head.payload_mode, DeliveryPayloadMode::EnvelopeMessage)
        && !head.relay_wide_target;
    let mut batch = vec![head];
    if !coalescable {
        return batch;
    }
    extend_batch_with_drain(&mut batch, drain_max, carry, receiver);
    batch
}

/// Drains carry-then-channel into an existing coalescable batch under the
/// head-coalesce predicate, up to `drain_max`. Used by `coalesce_batch` for the
/// pre-wait drain and by the worker loop for the post-quiescence drain that
/// absorbs tasks arriving while the per-target pane wait was in flight.
pub(super) fn extend_batch_with_drain(
    batch: &mut Vec<AsyncDeliveryTask>,
    drain_max: usize,
    carry: &mut VecDeque<AsyncDeliveryTask>,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
) {
    debug_assert!(!batch.is_empty(), "extend requires a non-empty head batch");
    while batch.len() < drain_max {
        let candidate = if let Some(task) = carry.pop_front() {
            Some(task)
        } else {
            receiver.try_recv().ok()
        };
        let Some(candidate) = candidate else {
            break;
        };
        if !can_coalesce_with_head(&batch[0], &candidate) {
            // Different mode or UI-routing: defer so the head of the next
            // iteration starts a fresh batch with this task.
            carry.push_front(candidate);
            break;
        }
        batch.push(candidate);
    }
}

/// Coalesce predicate: the candidate must share the head's payload mode and
/// UI-routing. Target session, runtime, and bundle are guaranteed identical
/// by the per-target worker registry key — assert in debug for safety.
fn can_coalesce_with_head(head: &AsyncDeliveryTask, candidate: &AsyncDeliveryTask) -> bool {
    debug_assert_eq!(head.target_session, candidate.target_session);
    debug_assert_eq!(head.runtime_directory, candidate.runtime_directory);
    debug_assert_eq!(head.bundle.bundle_name, candidate.bundle.bundle_name);
    matches!(head.payload_mode, DeliveryPayloadMode::EnvelopeMessage)
        && matches!(candidate.payload_mode, DeliveryPayloadMode::EnvelopeMessage)
        && head.relay_wide_target == candidate.relay_wide_target
}

/// Identifies a head task that needs the worker-loop tmux quiescence hoist.
/// Returns the cloned `TmuxTargetConfiguration` so the blocking pane wait can
/// run without re-borrowing into the worker's bundle state. ACP/UI targets
/// and RawInput heads return `None` — they keep their original transport flow.
fn classify_tmux_quiescence_hoist(task: &AsyncDeliveryTask) -> Option<TmuxTargetConfiguration> {
    if !matches!(task.payload_mode, DeliveryPayloadMode::EnvelopeMessage) || task.relay_wide_target
    {
        return None;
    }
    let target_member = task
        .bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session)?;
    match &target_member.target {
        TargetConfiguration::Tmux(tmux_target) => Some(tmux_target.clone()),
        _ => None,
    }
}

/// Fans a single failure template across every task in a coalesced batch,
/// preserving each task's own `message_id` / `target_session`, then completes
/// the tasks and releases their pending slots. Used when the worker-loop
/// quiescence hoist fails before paste begins so all coalesced tasks receive
/// the same outcome and the loop can continue.
fn complete_batch_with_template(
    batch: &[AsyncDeliveryTask],
    template: SendResult,
    pending: &std::sync::atomic::AtomicUsize,
) {
    for task in batch {
        let outcome = Ok(SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: template.outcome.clone(),
            reason_code: template.reason_code.clone(),
            reason: template.reason.clone(),
            details: template.details.clone(),
        });
        super::super::async_worker::complete_task_outcome(task, outcome);
        super::super::async_worker::release_pending_slot(pending);
    }
}

fn drain_carry_on_shutdown(
    carry: &mut VecDeque<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    while let Some(task) = carry.pop_front() {
        super::super::async_worker::complete_task_on_shutdown(&task);
        super::super::async_worker::release_pending_slot(pending);
    }
}

async fn bootstrap_acp_runtime_on_worker_start(
    key: &AsyncWorkerKey,
    bootstrap: AcpWorkerBootstrap,
) -> Option<PersistentAcpWorkerRuntime> {
    set_acp_worker_state(
        key.bundle_name.as_str(),
        bootstrap.runtime_directory.as_path(),
        bootstrap.target_member.id.as_str(),
        AcpWorkerReadinessState::Initializing,
    );
    let bundle_name = key.bundle_name.clone();
    let target_session = key.target_session.clone();
    let runtime_directory = bootstrap.runtime_directory.clone();
    let target_member = bootstrap.target_member.clone();
    let result = tokio::task::spawn_blocking(move || {
        bootstrap_acp_worker_runtime(runtime_directory.as_path(), &target_member)
    })
    .await
    .expect("ACP worker bootstrap task panicked");

    match result {
        Ok(runtime) => {
            install_acp_worker_replay_buffer(
                bundle_name.as_str(),
                bootstrap.runtime_directory.as_path(),
                bootstrap.target_member.id.as_str(),
                runtime.client.replay_buffer_handle(),
            );
            set_acp_worker_state(
                bundle_name.as_str(),
                bootstrap.runtime_directory.as_path(),
                bootstrap.target_member.id.as_str(),
                AcpWorkerReadinessState::Available,
            );
            Some(runtime)
        }
        Err(error) => {
            set_acp_worker_state(
                bundle_name.as_str(),
                bootstrap.runtime_directory.as_path(),
                bootstrap.target_member.id.as_str(),
                AcpWorkerReadinessState::Unavailable,
            );
            emit_inscription(
                "relay.acp.worker.bootstrap_failed",
                &json!({
                    "bundle_name": bundle_name,
                    "target_session": target_session,
                    "error_code": error.code,
                    "reason": error.reason,
                }),
            );
            None
        }
    }
}

/// Drives one batched delivery on the blocking pool. Moves the per-worker ACP
/// runtime into the blocking task and back out, so its sync state machine
/// never crosses an `.await`. Returns one outcome per accepted task plus any
/// ACP-peeled tasks that must be re-queued for the next worker iteration.
async fn deliver_batch_blocking(
    batch: Vec<AsyncDeliveryTask>,
    pre_resolved_pane: Option<String>,
    acp_runtime: Option<PersistentAcpWorkerRuntime>,
) -> (
    Vec<Result<SendResult, RelayError>>,
    Option<PersistentAcpWorkerRuntime>,
    Vec<AsyncDeliveryTask>,
) {
    tokio::task::spawn_blocking(move || {
        let mut local_runtime = acp_runtime;
        let (outcomes, deferred) = super::orchestration::deliver_batch_with_worker_state(
            &batch,
            pre_resolved_pane,
            &mut local_runtime,
        );
        (outcomes, local_runtime, deferred)
    })
    .await
    .expect("delivery blocking task panicked")
}

/// Awaits the per-target single-flight prompt completion on the blocking pool.
/// Returns the runtime so the worker loop retains ownership for the next task.
///
/// The wait is polled in bounded slices so process shutdown can abandon it
/// promptly: without this the worker's blocking thread could `recv()` forever
/// on an agent that never completes its turn, pinning the runtime's blocking
/// pool and blocking clean shutdown. On a shutdown abandon the worker returns
/// and drops its ACP runtime, whose `Drop` kills the child.
async fn wait_for_prompt_complete_blocking(
    acp_runtime: Option<PersistentAcpWorkerRuntime>,
) -> Option<PersistentAcpWorkerRuntime> {
    tokio::task::spawn_blocking(move || {
        if let Some(runtime) = acp_runtime.as_ref() {
            while !runtime
                .client
                .wait_for_prompt_complete(ACP_PROMPT_WAIT_POLL_INTERVAL)
            {
                if shutdown_requested() {
                    break;
                }
            }
        }
        acp_runtime
    })
    .await
    .expect("ACP prompt-complete wait task panicked")
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

fn respawn_backoff_cap_ms() -> u64 {
    std::env::var(RESPAWN_BACKOFF_MAX_MS_ENVVAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(RESPAWN_BACKOFF_CAP_DEFAULT_MS)
}

fn classify_respawn_trigger(outcome: &Result<SendResult, RelayError>) -> &'static str {
    // All tasks in a coalesced batch share one transport outcome by
    // construction, so classifying off the first outcome is sufficient.
    match outcome {
        Ok(result) => match result.reason_code.as_deref() {
            Some(code) if code == ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE => "transport_unavailable",
            Some(code) if code == ACP_ERROR_CODE_PROMPT_FAILED => "serialization_failed",
            Some(code) if code == ACP_ERROR_CODE_CONNECTION_CLOSED => "connection_closed",
            _ => "worker_unavailable",
        },
        Err(_) => "worker_unavailable",
    }
}

async fn drive_acp_worker_respawn(
    key: &AsyncWorkerKey,
    ctx: &AcpWorkerBootstrap,
    trigger_reason: &'static str,
    respawn_state: &mut AcpRespawnState,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) {
    // Drop the dead runtime so its child and reader thread are joined before
    // the new child is spawned. Without this, `respawn_acp_worker_runtime`
    // would leave the zombie process unreaped until the worker task exits.
    *acp_runtime = None;

    loop {
        if shutdown_requested() {
            return;
        }
        let backoff = respawn_state.advance();
        set_acp_worker_state(
            key.bundle_name.as_str(),
            ctx.runtime_directory.as_path(),
            ctx.target_member.id.as_str(),
            AcpWorkerReadinessState::Recovering,
        );
        emit_inscription(
            "relay.acp.respawn.triggered",
            &json!({
                "bundle_name": key.bundle_name,
                "target_session": ctx.target_member.id,
                "attempt": respawn_state.attempt,
                "trigger_reason": trigger_reason,
                "backoff_ms": backoff.as_millis() as u64,
            }),
        );
        broadcast_event_to_bundle_ui(
            key.bundle_name.as_str(),
            &acp_respawn_stream_event(
                "acp_worker_respawn_started",
                key.bundle_name.as_str(),
                ctx.target_member.id.as_str(),
                json!({
                    "attempt": respawn_state.attempt,
                    "trigger_reason": trigger_reason,
                    "backoff_ms": backoff.as_millis() as u64,
                }),
            ),
        );

        if !sleep_with_shutdown_gate(backoff).await {
            return;
        }

        let permission_context = PermissionEventContext {
            runtime_directory: ctx.runtime_directory.clone(),
            bundle_name: key.bundle_name.clone(),
            authorized_ui_sessions: list_registered_ui_sessions_for_bundle(
                key.bundle_name.as_str(),
            ),
        };
        if let Err(reason) =
            invalidate_pending_for_respawn(&permission_context, ctx.target_member.id.as_str())
        {
            emit_inscription(
                "relay.acp.respawn.permission_invalidate_failed",
                &json!({
                    "bundle_name": key.bundle_name,
                    "target_session": ctx.target_member.id,
                    "reason": reason,
                }),
            );
        }

        let respawn_bundle_name = key.bundle_name.clone();
        let respawn_runtime_directory = ctx.runtime_directory.clone();
        let respawn_target_member = ctx.target_member.clone();
        let respawn_result = tokio::task::spawn_blocking(move || {
            respawn_acp_worker_runtime(
                respawn_bundle_name.as_str(),
                respawn_runtime_directory.as_path(),
                &respawn_target_member,
            )
        })
        .await
        .expect("ACP respawn task panicked");

        match respawn_result {
            Ok(runtime) => {
                set_acp_worker_state(
                    key.bundle_name.as_str(),
                    ctx.runtime_directory.as_path(),
                    ctx.target_member.id.as_str(),
                    AcpWorkerReadinessState::Available,
                );
                emit_inscription(
                    "relay.acp.respawn.succeeded",
                    &json!({
                        "bundle_name": key.bundle_name,
                        "target_session": ctx.target_member.id,
                        "attempt": respawn_state.attempt,
                    }),
                );
                broadcast_event_to_bundle_ui(
                    key.bundle_name.as_str(),
                    &acp_respawn_stream_event(
                        "acp_worker_respawn_completed",
                        key.bundle_name.as_str(),
                        ctx.target_member.id.as_str(),
                        json!({
                            "attempt": respawn_state.attempt,
                            "outcome": "succeeded",
                        }),
                    ),
                );
                *acp_runtime = Some(runtime);
                respawn_state.reset_on_success();
                return;
            }
            Err(error) => {
                respawn_state.record_failure(&error);
                emit_inscription(
                    "relay.acp.respawn.attempt_failed",
                    &json!({
                        "bundle_name": key.bundle_name,
                        "target_session": ctx.target_member.id,
                        "attempt": respawn_state.attempt,
                        "error_code": error.code,
                        "reason": error.reason,
                    }),
                );
                if error.is_permanent() || respawn_state.should_give_up() {
                    set_acp_worker_state(
                        key.bundle_name.as_str(),
                        ctx.runtime_directory.as_path(),
                        ctx.target_member.id.as_str(),
                        AcpWorkerReadinessState::Unavailable,
                    );
                    emit_inscription(
                        "relay.acp.respawn.permanent_failure",
                        &json!({
                            "bundle_name": key.bundle_name,
                            "target_session": ctx.target_member.id,
                            "attempts": respawn_state.attempt,
                            "final_error_code": error.code,
                            "reason": error.reason,
                        }),
                    );
                    broadcast_event_to_bundle_ui(
                        key.bundle_name.as_str(),
                        &acp_respawn_stream_event(
                            "acp_worker_respawn_completed",
                            key.bundle_name.as_str(),
                            ctx.target_member.id.as_str(),
                            json!({
                                "attempts": respawn_state.attempt,
                                "outcome": "permanent_failure",
                                "final_error_code": error.code,
                                "reason": error.reason,
                            }),
                        ),
                    );
                    return;
                }
            }
        }
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

#[cfg(test)]
mod coalesce_batch_tests {
    //! Locality-justified inline coverage: `coalesce_batch` operates over
    //! crate-private types (`AsyncDeliveryTask`, `DeliveryPayloadMode`,
    //! `QuiescenceOptions`) that aren't part of any stable contract, so
    //! external `tests/` files cannot name them without a `#[doc(hidden)] pub`
    //! escape hatch that would be more API surface than this is worth. The
    //! coalesce loop's branches are all reachable from the heterogeneous
    //! queue case the Coordinator named; the rest are obvious from the code.
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::configuration::{
        BundleConfiguration, BundleMember, TargetConfiguration, TmuxTargetConfiguration,
    };
    use crate::envelope::PromptBatchSettings;
    use crate::relay::delivery::QuiescenceOptions;

    fn task(message_id: &str, payload_mode: DeliveryPayloadMode) -> AsyncDeliveryTask {
        let member = BundleMember {
            id: "bravo".to_string(),
            name: None,
            working_directory: None,
            target: TargetConfiguration::Tmux(TmuxTargetConfiguration {
                start_command: "sh -c 'exit 0'".to_string(),
                prompt_readiness: None,
            }),
            coder_session_id: None,
            policy_id: None,
        };
        AsyncDeliveryTask {
            bundle: BundleConfiguration {
                schema_version: "1".to_string(),
                bundle_name: "party".to_string(),
                autostart: false,
                groups: Vec::new(),
                members: vec![member.clone()],
            },
            sender_bundle_name: "party".to_string(),
            sender: member.clone(),
            authenticated_identity: None,
            all_target_sessions: vec!["bravo".to_string()],
            target_session: "bravo".to_string(),
            relay_wide_target: false,
            message: String::new(),
            message_id: message_id.to_string(),
            quiescence: QuiescenceOptions {
                quiet_window: Duration::from_millis(1),
                quiescence_timeout: Some(Duration::from_millis(1)),
                acp_turn_timeout_override: None,
            },
            batch_settings: PromptBatchSettings::default(),
            runtime_directory: PathBuf::from("/tmp/relay-test"),
            completion_sender: None,
            payload_mode,
            append_enter: true,
            permission_decider_sessions: Vec::new(),
            permission_max_pending: 0,
        }
    }

    /// Pre- and post-quiescence drain regression across one heterogeneous
    /// stream. Simulates the worker loop without the tmux pane wait by driving
    /// `coalesce_batch` (pre-wait) and then `extend_batch_with_drain` directly
    /// (post-wait) against a single `UnboundedReceiver`.
    ///
    /// Stream timeline:
    /// - `e1` is queued first → becomes head of the first iteration.
    /// - `r2` arrives before the pre-wait drain → must end the first batch
    ///   (mode-divergent) and head the second iteration.
    /// - `e3` arrives before the second pre-wait drain → becomes the head of
    ///   iteration three.
    /// - `e4`, `e5` arrive *during* iteration three's quiescence wait → the
    ///   post-wait drain must absorb them into the same batch (the operator-
    ///   observed bug case: drained_count=3 with pre=1/post=2).
    ///
    /// One test covers every coalesce-batch branch the slice introduces
    /// (head-is-envelope, head-is-raw skip, mode-change carry pushback,
    /// multi-task pack) plus the post-wait extension being the same helper.
    #[test]
    fn pre_and_post_quiescence_drains_cover_heterogeneous_stream() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        runtime.block_on(async move {
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut carry: VecDeque<AsyncDeliveryTask> = VecDeque::new();
            let mut iterations: Vec<Vec<String>> = Vec::new();

            // Iteration 1: pre-wait drain only — [e1].
            sender
                .send(task("e1", DeliveryPayloadMode::EnvelopeMessage))
                .expect("seed e1");
            sender
                .send(task("r2", DeliveryPayloadMode::RawInput))
                .expect("seed r2");
            let head = receiver.try_recv().expect("e1 head");
            let batch = coalesce_batch(head, 32, &mut carry, &mut receiver);
            iterations.push(batch.iter().map(|t| t.message_id.clone()).collect());

            // Iteration 2: RawInput head, pre-wait drain returns it alone.
            let head = carry
                .pop_front()
                .or_else(|| receiver.try_recv().ok())
                .expect("r2 head");
            let batch = coalesce_batch(head, 32, &mut carry, &mut receiver);
            iterations.push(batch.iter().map(|t| t.message_id.clone()).collect());

            // Iteration 3: envelope head, *post-wait* drain absorbs e4/e5
            // that landed in the channel while the quiescence wait was in
            // flight (simulated by sending after coalesce_batch returns).
            sender
                .send(task("e3", DeliveryPayloadMode::EnvelopeMessage))
                .expect("seed e3");
            let head = receiver.try_recv().expect("e3 head");
            let mut batch = coalesce_batch(head, 32, &mut carry, &mut receiver);
            let pre_quiescence_count = batch.len();
            // Tasks arriving during the (skipped) quiescence wait:
            sender
                .send(task("e4", DeliveryPayloadMode::EnvelopeMessage))
                .expect("seed e4");
            sender
                .send(task("e5", DeliveryPayloadMode::EnvelopeMessage))
                .expect("seed e5");
            extend_batch_with_drain(&mut batch, 32, &mut carry, &mut receiver);
            assert_eq!(pre_quiescence_count, 1, "pre-wait batch should be [e3]");
            assert_eq!(
                batch.len() - pre_quiescence_count,
                2,
                "post-wait drain should absorb e4 and e5",
            );
            iterations.push(batch.iter().map(|t| t.message_id.clone()).collect());

            assert_eq!(
                iterations,
                vec![
                    vec!["e1".to_string()],
                    vec!["r2".to_string()],
                    vec!["e3".to_string(), "e4".to_string(), "e5".to_string()],
                ],
            );
            assert!(carry.is_empty(), "no tasks should be stranded in carry");
            assert!(
                receiver.try_recv().is_err(),
                "no tasks should be left in the channel",
            );
        });
    }
}
