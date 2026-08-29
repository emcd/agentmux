use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::json;

use std::path::{Path, PathBuf};

use crate::acp::AcpTransport;
use crate::acp::persistent_runtime::{AcpBootstrapError, bootstrap_acp_worker_runtime};
use crate::configuration::BundleMember;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::signals::shutdown_requested;
use crate::transports::{UnreachableSince, WorkerFailureReason, WorkerReadinessState};

use super::services::{AcpDriverServices, MirrorStateFn};

const RESPAWN_BACKOFF_MAX_MS_ENVVAR: &str = "AGENTMUX_RELAY_ACP_RESPAWN_BACKOFF_MAX_MS";
const RESPAWN_SLEEP_POLL_MS: u64 = 50;
const RESPAWN_BACKOFF_INITIAL_MS: u64 = 1_000;
const RESPAWN_BACKOFF_CAP_DEFAULT_MS: u64 = 30_000;
const RESPAWN_ATTEMPT_LIMIT: u32 = 6;
const RESPAWN_MONITOR_POLL_MS: u64 = 100;
const RESPAWN_TRIGGER_REASON: &str = "worker_unavailable";

enum BootstrapDisposition {
    /// The runtime is live and installed on the transport.
    Installed,
    /// A fence began while the bootstrap was running, so the runtime was refused
    /// and shut down inside the bootstrap closure.
    RefusedAfterFence,
    /// No runtime was produced.
    Failed(AcpBootstrapError),
}

/// Runs one bootstrap on the blocking pool, disposes of the runtime it produces
/// — installing it, or tearing it down when a fence refuses it — and publishes
/// the readiness that disposal leaves behind, all inside that same closure.
///
/// None of that can be left to the task awaiting the result. A bootstrap runs on
/// a blocking pool, and `tokio`'s abort cancels only the awaiting task, never
/// the closure; a bootstrap that finishes after such an abort would hand a live
/// agent child to nobody and would leave the target advertising a runtime that
/// is still on its way. So the split is by what the work belongs to rather than
/// by what is convenient: everything that must happen no matter who is still
/// listening lives here, and only the retry policy — backoff state, per-attempt
/// inscriptions — stays with the caller. Owning creation through
/// install-or-teardown here is also what makes the in-flight count mean
/// something: it spans the runtime's whole exposure rather than stopping at the
/// moment it was constructed, when nothing had yet decided the child's fate.
///
/// A failed attempt publishes nothing, because whether it is terminal is exactly
/// the retry policy's question.
///
/// The transport lock is taken only for the install decision, which is a few
/// field updates and a thread spawn — never across the child spawn or the ACP
/// handshake, so a concurrent `mailw` is not stalled by a bootstrap.
async fn run_one_bootstrap(
    transport: &Arc<Mutex<AcpTransport>>,
    mirror_state: &MirrorStateFn,
    runtime_directory: PathBuf,
    target_member: BundleMember,
) -> BootstrapDisposition {
    let in_flight = transport
        .lock()
        .expect("acp transport mutex poisoned")
        .begin_bootstrap();
    let transport = Arc::clone(transport);
    let mirror_state = Arc::clone(mirror_state);
    tokio::task::spawn_blocking(move || {
        let publish = |generation| in_flight.publish_generation(generation);
        match bootstrap_acp_worker_runtime(&runtime_directory, &target_member, &publish) {
            Ok(runtime) => {
                // The check and the install happen under one lock: a fence that
                // began while this bootstrap ran must not find a fresh agent
                // installed into a generation it has already declared stopped.
                let refused = transport
                    .lock()
                    .expect("acp transport mutex poisoned")
                    .install_runtime_unless_fenced(runtime);
                match refused {
                    Some(mut refused) => {
                        refused.client.shutdown();
                        // The target has no runtime and a fenced generation
                        // admits no replacement, so Unavailable is the accurate
                        // terminal state. Leaving it Initializing or Recovering
                        // told every observer — the TUI, the startup poller, a
                        // `list` — that a runtime was still on its way.
                        mirror_state(WorkerReadinessState::Unavailable);
                        BootstrapDisposition::RefusedAfterFence
                    }
                    None => {
                        mirror_state(WorkerReadinessState::Available);
                        BootstrapDisposition::Installed
                    }
                }
            }
            Err(error) => BootstrapDisposition::Failed(error),
        }
    })
    .await
    .expect("ACP bootstrap task panicked")
}

/// The driver-owned initial bootstrap: establishes the first runtime for a
/// target and reports the outcome through the relay touchpoints.
///
/// Runs as its own task so the relay worker's loop — and therefore its shutdown
/// gate — is reachable throughout. `settled` is raised on every path that
/// returns, so the worker knows when its queue may start flowing; a bootstrap
/// aborted by a fence never raises it, which is correct, because that worker is
/// draining rather than delivering.
pub(super) async fn initial_acp_bootstrap(
    transport: Arc<Mutex<AcpTransport>>,
    services: AcpDriverServices,
    namespace: String,
    runtime_directory: PathBuf,
    target_member: BundleMember,
    abandonment: AbandonmentSignal,
    settled: Arc<AtomicBool>,
) {
    let target_session = target_member.id.clone();
    let disposition = run_one_bootstrap(
        &transport,
        &services.mirror_state,
        runtime_directory,
        target_member,
    )
    .await;
    match disposition {
        BootstrapDisposition::Installed => {}
        BootstrapDisposition::RefusedAfterFence => {
            emit_inscription(
                "relay.acp.worker.bootstrap_refused_after_fence",
                &json!({
                    "namespace": namespace,
                    "target_session": target_session,
                }),
            );
        }
        BootstrapDisposition::Failed(error) => {
            transport
                .lock()
                .expect("acp transport mutex poisoned")
                .mark_runtime_unavailable();
            // Record the failure before the readiness transition so the startup
            // poller, which acts the moment it observes Unavailable, finds the
            // true cause already stored.
            (services.record_failure)(WorkerFailureReason {
                code: error.code.clone(),
                reason: error.reason.clone(),
            });
            (services.mirror_state)(WorkerReadinessState::Unavailable);
            emit_inscription(
                "relay.acp.worker.bootstrap_failed",
                &json!({
                    "namespace": namespace,
                    "target_session": target_session,
                    "error_code": error.code,
                    "reason": error.reason,
                }),
            );
            if error.is_permanent() {
                // A permanent bootstrap failure gets no respawn signal: the
                // monitor would only run one more attempt to discover the same
                // permanence and abandon. Latch abandonment here instead, so the
                // health axis reads unreachable immediately and the dwell clock
                // starts at the transition rather than at a later enquiry.
                abandonment.abandoned.store(true, Ordering::Release);
                let _ = abandonment.unreachable_since.fold(false);
                emit_inscription(
                    "relay.acp.respawn.permanent_failure",
                    &json!({
                        "namespace": namespace,
                        "target_session": target_session,
                        "attempts": 1,
                        "final_error_code": error.code,
                        "reason": error.reason,
                    }),
                );
                (services.broadcast_ui)(
                    "acp_worker_respawn_completed",
                    json!({
                        "attempts": 1,
                        "outcome": "permanent_failure",
                        "final_error_code": error.code,
                        "reason": error.reason,
                    }),
                );
            } else {
                // No delivery task is running to emit the respawn-needed signal,
                // so prime it directly: the monitor will retry with backoff.
                transport
                    .lock()
                    .expect("acp transport mutex poisoned")
                    .signal_respawn();
            }
        }
    }
    settled.store(true, Ordering::Release);
}

/// Driver-owned async respawn monitor. Subscribes to the transport's stable
/// respawn-needed signal and drives respawn off the relay worker loop. The
/// transport is shared via `Arc<Mutex<AcpTransport>>`; the monitor locks only for
/// the fast release/install steps — never across `.await` or the blocking child
/// spawn — so a concurrent worker `mailw` is never stalled. Exits on relay
/// shutdown.
/// The target one respawn monitor supervises.
///
/// Grouped rather than passed as three parallel parameters: they are one
/// cohesive identity — which target, where its runtime lives, and how it is
/// configured — and every re-establish attempt needs all three together.
/// Whether the monitor owes this target a respawn attempt.
///
/// Extracted from the monitor loop because the interesting cases are
/// *combinations* — a signal without an `Unavailable` runtime, an `Unavailable`
/// runtime without a signal — and staging those against a live monitor means
/// racing a respawn to observe a state it is about to leave.
fn respawn_is_owed(signalled: bool, abandoned: bool, readiness: WorkerReadinessState) -> bool {
    signalled && !abandoned && matches!(readiness, WorkerReadinessState::Unavailable)
}

/// The health signals a respawn (or a permanent initial-bootstrap failure)
/// writes when it gives up on a target.
///
/// Paired rather than passed separately because they are one fact recorded in
/// two places — that the target is past recovery, and when that became true —
/// and separating them is exactly how the instant came to be stamped at first
/// enquiry instead of at the transition. Owned so the pair can be handed to the
/// spawned bootstrap task alongside the respawn monitor.
pub(super) struct AbandonmentSignal {
    pub(super) abandoned: Arc<AtomicBool>,
    pub(super) unreachable_since: Arc<UnreachableSince>,
}

pub(super) struct AcpRespawnTarget {
    pub(super) namespace: String,
    pub(super) runtime_directory: PathBuf,
    pub(super) member: BundleMember,
}

pub(super) async fn acp_respawn_monitor(
    transport: Arc<Mutex<AcpTransport>>,
    mut respawn_needed: tokio::sync::watch::Receiver<u64>,
    services: AcpDriverServices,
    mut respawn_state: AcpRespawnState,
    target: AcpRespawnTarget,
    respawn_abandoned: Arc<AtomicBool>,
    unreachable_since: Arc<UnreachableSince>,
) {
    let poll = Duration::from_millis(RESPAWN_MONITOR_POLL_MS);
    loop {
        tokio::select! {
            biased;
            changed = respawn_needed.changed() => {
                if changed.is_err() {
                    // All senders dropped: the transport is gone.
                    return;
                }
            }
            _ = tokio::time::sleep(poll) => {}
        }
        if shutdown_requested() {
            return;
        }
        // The fence's cooperative request. A fenced generation admits no
        // replacement, so there is nothing left for this monitor to do and
        // continuing would only race the install check.
        if transport
            .lock()
            .expect("acp transport mutex poisoned")
            .generation_is_fenced()
        {
            return;
        }
        // Three conditions, each excluding something the others cannot.
        //
        // The **signal** carries the classification. Not every `Unavailable` is
        // a respawn: the delivery task raises the signal only on a connection
        // close or transport write failure, and deliberately not on a
        // non-delivery like serialization failure, which is not recoverable by
        // restarting the agent. A level-only trigger would respawn on it and
        // override that judgement.
        //
        // The **level** guards against staleness. A producer's edge can arrive
        // after the runtime it described has already been replaced, and acting
        // on it would tear down a healthy generation to recover from a failure
        // that is already over.
        //
        // **Abandonment** guards the crash loop that `RESPAWN_ATTEMPT_LIMIT`
        // exists to stop.
        //
        // What makes this independent of delivery traffic is not a level-only
        // trigger but the signal's *persistence*: it is cleared below only once
        // the runtime is no longer `Unavailable`, so a failed attempt leaves it
        // standing and the monitor retries on its own clock. Re-priming from
        // `mailw` — recovery only because something tried to write — is what
        // that replaces.
        respawn_needed.borrow_and_update();
        let abandoned = respawn_abandoned.load(Ordering::Acquire);
        let (outstanding, readiness) = {
            let transport = transport.lock().expect("acp transport mutex poisoned");
            (
                transport.respawn_signal_outstanding(),
                transport.readiness(),
            )
        };
        if !respawn_is_owed(outstanding.is_some(), abandoned, readiness) {
            // Once raised, "owed" and "answered" are exact complements: owed is
            // `!abandoned && Unavailable`, so an outstanding cause that is not
            // owed has necessarily been answered -- the runtime left
            // `Unavailable` under its own power, or abandonment closed the
            // account.
            //
            // Retiring it here and not only after this monitor's own attempt is
            // what keeps a cause from outliving the failure it described. One
            // left standing across a recovery stays outstanding, and the next
            // `Unavailable` -- including a serialization failure, which the
            // delivery task deliberately does not signal -- would find all
            // three conditions met and respawn on a classification that was
            // never made for it. The level check cannot catch that on its own:
            // it distinguishes states, not which failure put the runtime in
            // one.
            //
            // Retiring *this* epoch rather than clearing the signal is what
            // makes the decision safe to act on late. Everything above is a
            // sample: the lock is released before the retirement lands, so a
            // live failure can publish a new cause in between. That cause
            // carries a higher epoch, so the retirement bounds only what was
            // classified and leaves the new one outstanding for the next tick.
            if let Some(epoch) = outstanding {
                transport
                    .lock()
                    .expect("acp transport mutex poisoned")
                    .retire_respawn_signal(epoch);
            }
            continue;
        }
        run_acp_respawn(
            &transport,
            &services,
            &mut respawn_state,
            target.namespace.as_str(),
            target.runtime_directory.as_path(),
            &target.member,
            AbandonmentSignal {
                abandoned: Arc::clone(&respawn_abandoned),
                unreachable_since: Arc::clone(&unreachable_since),
            },
        )
        .await;
        // Retire the cause only once it has been answered — the runtime is no
        // longer `Unavailable`, or respawn has been abandoned and no further
        // attempt is coming. Retiring unconditionally is what made an external
        // re-prime necessary: an attempt that failed without exhausting the
        // budget left the worker dead with nothing left to say so, and recovery
        // then waited for a write that the readiness gate would never allow.
        //
        // Holding the cause while `Unavailable` persists also keeps the
        // classification intact across retries. It was raised because *this*
        // failure warranted a respawn; that judgement does not expire because an
        // attempt did not take.
        //
        // The epoch retired is the one this attempt answered, never whatever is
        // current. A respawn runs for a while, and a failure on the way back up
        // can publish a cause of its own; bounding by the classified epoch
        // leaves that one for the next tick instead of consuming it here.
        let answered = {
            let transport = transport.lock().expect("acp transport mutex poisoned");
            !matches!(transport.readiness(), WorkerReadinessState::Unavailable)
        } || respawn_abandoned.load(Ordering::Acquire);
        if answered {
            let epoch = outstanding.expect("an owed respawn has an outstanding cause");
            transport
                .lock()
                .expect("acp transport mutex poisoned")
                .retire_respawn_signal(epoch);
        }
    }
}

/// Releases the dead runtime and re-establishes it with capped exponential
/// backoff, mirroring Recovering/Available/Unavailable transitions, broadcasting
/// respawn stream events, and invalidating pending choices before each attempt.
/// Returns when re-establish succeeds, the failure is permanent, the retry budget
/// is exhausted, or shutdown is requested. The blocking child spawn runs off the
/// transport lock; only the fast release/install steps hold it.
async fn run_acp_respawn(
    transport: &Arc<Mutex<AcpTransport>>,
    services: &AcpDriverServices,
    respawn_state: &mut AcpRespawnState,
    namespace: &str,
    runtime_directory: &Path,
    target_member: &BundleMember,
    abandonment: AbandonmentSignal,
) {
    let target_session = target_member.id.as_str();
    // Release the dead runtime (joining its child + reader thread) but keep the
    // transport and its published handle, marking it Recovering. A look racing the
    // respawn reads a recovering/stale snapshot through the still-valid handle.
    transport
        .lock()
        .expect("acp transport mutex poisoned")
        .release_runtime();

    loop {
        if shutdown_requested() {
            return;
        }
        let backoff = respawn_state.advance();
        (services.mirror_state)(WorkerReadinessState::Recovering);
        emit_inscription(
            "relay.acp.respawn.triggered",
            &json!({
                "namespace": namespace,
                "target_session": target_session,
                "attempt": respawn_state.attempt,
                "trigger_reason": RESPAWN_TRIGGER_REASON,
                "backoff_ms": backoff.as_millis() as u64,
            }),
        );
        (services.broadcast_ui)(
            "acp_worker_respawn_started",
            json!({
                "attempt": respawn_state.attempt,
                "trigger_reason": RESPAWN_TRIGGER_REASON,
                "backoff_ms": backoff.as_millis() as u64,
            }),
        );

        if !sleep_with_shutdown_gate(backoff).await {
            return;
        }

        (services.invalidate_choices)();

        // Set chooser/target + clear the prior channel under a brief lock; the
        // chooser is already set from initial startup, re-set for safety.
        transport
            .lock()
            .expect("acp transport mutex poisoned")
            .prepare_for_startup(services.chooser.clone(), target_member.id.clone());

        // Bootstrap the new runtime OFF the lock (blocking child spawn). The
        // install happens inside that same closure, so the published handle
        // stays valid (install repoints its replay slot) and a fence that began
        // mid-bootstrap refuses the runtime and tears it down there.
        let disposition = run_one_bootstrap(
            transport,
            &services.mirror_state,
            runtime_directory.to_path_buf(),
            target_member.clone(),
        )
        .await;

        match disposition {
            BootstrapDisposition::RefusedAfterFence => {
                emit_inscription(
                    "relay.acp.respawn.refused_after_fence",
                    &json!({
                        "namespace": namespace,
                        "target_session": target_session,
                        "attempt": respawn_state.attempt,
                    }),
                );
                return;
            }
            BootstrapDisposition::Installed => {
                emit_inscription(
                    "relay.acp.respawn.succeeded",
                    &json!({
                        "namespace": namespace,
                        "target_session": target_session,
                        "attempt": respawn_state.attempt,
                    }),
                );
                (services.broadcast_ui)(
                    "acp_worker_respawn_completed",
                    json!({
                        "attempt": respawn_state.attempt,
                        "outcome": "succeeded",
                    }),
                );
                respawn_state.reset_on_success();
                return;
            }
            BootstrapDisposition::Failed(error) => {
                emit_inscription(
                    "relay.acp.respawn.attempt_failed",
                    &json!({
                        "namespace": namespace,
                        "target_session": target_session,
                        "attempt": respawn_state.attempt,
                        "error_code": error.code,
                        "reason": error.reason,
                    }),
                );
                if error.is_permanent() || respawn_state.should_give_up() {
                    // Latch the health axis here, at the one place that knows no
                    // further attempt is coming. Every other route to
                    // `Unavailable` is survivable, so this is the only signal
                    // that separates a target worth waiting for from one that
                    // will never come back.
                    // Both halves of the same fact, recorded together: that this
                    // target is past recovery, and when that became true. Stamping
                    // the instant here rather than leaving it to whenever a member
                    // next asks is what keeps the dwell measuring how long the
                    // target has been unreachable — starting the clock on first
                    // enquiry would charge a late arrival a full fresh dwell for a
                    // target already known to be gone.
                    abandonment.abandoned.store(true, Ordering::Release);
                    let _ = abandonment.unreachable_since.fold(false);
                    transport
                        .lock()
                        .expect("acp transport mutex poisoned")
                        .mark_runtime_unavailable();
                    (services.record_failure)(WorkerFailureReason {
                        code: error.code.clone(),
                        reason: error.reason.clone(),
                    });
                    (services.mirror_state)(WorkerReadinessState::Unavailable);
                    emit_inscription(
                        "relay.acp.respawn.permanent_failure",
                        &json!({
                            "namespace": namespace,
                            "target_session": target_session,
                            "attempts": respawn_state.attempt,
                            "final_error_code": error.code,
                            "reason": error.reason,
                        }),
                    );
                    (services.broadcast_ui)(
                        "acp_worker_respawn_completed",
                        json!({
                            "attempts": respawn_state.attempt,
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

pub(super) struct AcpRespawnState {
    attempt: u32,
    next_backoff_ms: u64,
}

impl AcpRespawnState {
    pub(super) fn new() -> Self {
        Self {
            attempt: 0,
            next_backoff_ms: 0,
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

    fn should_give_up(&self) -> bool {
        self.attempt >= RESPAWN_ATTEMPT_LIMIT
    }

    fn reset_on_success(&mut self) {
        self.attempt = 0;
        self.next_backoff_ms = 0;
    }
}

#[cfg(test)]
mod respawn_owed_tests {
    use super::{WorkerReadinessState, respawn_is_owed};

    /// The three conditions each exclude something the others cannot, so the
    /// matrix is the assertion.
    ///
    /// Crate-private by design: the owed condition is monitor internals with no
    /// public consumer, and widening it to reach from `tests/unit` would add API
    /// surface that exists only for this check.
    #[test]
    fn a_respawn_is_owed_only_when_signal_level_and_budget_agree() {
        // The positive case: a failure that warranted a respawn, a runtime still
        // dead, and a budget that has not run out.
        assert!(respawn_is_owed(
            true,
            false,
            WorkerReadinessState::Unavailable
        ));

        // A stale edge. A producer's signal can arrive after the runtime it
        // described has already been replaced; acting on it would tear down a
        // healthy generation to recover from a failure that is already over. The
        // level is what makes the edge safe to receive late.
        for recovered in [
            WorkerReadinessState::Available,
            WorkerReadinessState::Busy,
            WorkerReadinessState::Recovering,
            WorkerReadinessState::Initializing,
        ] {
            assert!(
                !respawn_is_owed(true, false, recovered),
                "a signal must not respawn a runtime that is no longer Unavailable: {recovered:?}"
            );
        }

        // An Unavailable runtime with no signal. Not every Unavailable warrants a
        // respawn -- the delivery task raises the signal only on a connection
        // close or transport write failure, and never for a non-delivery like
        // serialization failure, which is not fixed by restarting the agent. The
        // signal is where that judgement lives, so a level-only trigger would
        // override it.
        assert!(
            !respawn_is_owed(false, false, WorkerReadinessState::Unavailable),
            "an Unavailable the transport did not signal for is not a respawn"
        );

        // Abandoned: past recovery whatever else is true. This is the crash loop
        // `RESPAWN_ATTEMPT_LIMIT` exists to stop.
        assert!(!respawn_is_owed(
            true,
            true,
            WorkerReadinessState::Unavailable
        ));
    }
}
