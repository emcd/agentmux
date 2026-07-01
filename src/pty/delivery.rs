//! Pty worker delivery state machine.
//!
//! The Pty worker thread is the only thread that can apply bytes to
//! the libghostty-vt terminal (the terminal is `!Send + !Sync`). A
//! blocking wait that does not drain `bytes_rx` (PTY output) during
//! the wait would leave the terminal showing the pre-wait screen
//! state even after child output arrives — the quiescence probe would
//! never observe the response, and the wait would fire `Timeout` /
//! `Wedged` or hang indefinitely.
//!
//! [`DeliveryRun`] wraps the cross-transport state machine
//! ([`quiescence_classify_step`]) and drives it one step at a time.
//! Between steps the worker event loop services other channels
//! (snapshot requests, PTY byte drain, write_rx absorption). The
//! step's wait helper ([`PtyDeliveryWait::run`]) drains `bytes_rx` so
//! the terminal reflects the latest child output before the next
//! observation, AND absorbs additional `Envelope` commands from
//! `write_rx` into the current group (coalesce-during-wait). A `Raw`
//! command arriving during the wait is stashed in `pending_raw` so
//! the worker can resolve the current group and then process the
//! raw as a fresh delivery (batch-barrier semantics).

use std::{
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::{mpsc, oneshot};

use crate::transports::{
    DeliveryEnvelope, DeliveryWaitError, QuiescenceAction, QuiescenceState, SingleDeliveryOutcome,
    quiescence_classify_step,
};

use super::state::{PtyShared, WorkerTerminalProbe};

const QUIET_WINDOW: Duration = Duration::from_millis(50);
/// Poll interval for the worker wait loop. Short enough that the
/// state machine responds quickly to incoming bytes; long enough that
/// an idle wait does not burn CPU.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A `Raw` command waiting to be processed by the next delivery.
/// Held on the `DeliveryRun` between worker iterations when a batch
/// barrier was hit (either during coalesce-before-wait or during
/// the wait itself).
pub struct PendingRaw {
    pub content: String,
    pub append_enter: bool,
    pub outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
}

/// One in-flight delivery on the worker thread. Owns the coalesced
/// envelope group and the per-delivery state-machine state.
pub struct DeliveryRun {
    /// The coalesced group of envelopes. All members share the wait
    /// outcome base; each sender keeps its own `message_id`. Drained
    /// (and its senders consumed) when the run resolves.
    group: Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    /// Quiet window for quiescence detection. 50ms matches the Tmux
    /// transport.
    quiet_window: Duration,
    /// Prime-timeout anchor (the instant `start` was called). Used
    /// for diagnostic `prime_wait_elapsed_ms` values.
    prime_started_at: Instant,
    /// Prime-timeout deadline. `None` means unbounded.
    prime_deadline: Option<Instant>,
    /// Configured prime-timeout value (used for diagnostic inscriptions).
    prime_timeout_ms: Option<u64>,
    /// Whether wedge detection is enabled.
    wedge_detection: bool,
    /// Cross-transport state machine state (wedge counter + last
    /// emitted mismatch signature).
    qstate: QuiescenceState,
    /// Set to `true` once the state machine resolves. The worker
    /// drops the run and returns to idle.
    resolved: bool,
}

/// Outcome of one [`DeliveryRun::step`] call. The worker loop uses
/// this to drive its event loop. Outcomes for the current group are
/// sent inside `step` (each sender in the coalesced group is
/// consumed with its per-message-id outcome); the variant tells the
/// worker whether the run is done, still in progress, or hit a
/// batch barrier (raw pending).
#[derive(Debug, PartialEq, Eq)]
pub enum DeliveryStep {
    /// The delivery is still in progress. The worker should
    /// re-service channels and call `step` again.
    Continue,
    /// The delivery has resolved. The current group's outcomes have
    /// been sent to each sender. The worker should drop the run and
    /// return to idle.
    Done,
}

/// One Pty-specific wait step. Drains `bytes_rx` (apply to terminal,
/// advance the change atomic) and absorbs envelopes from `write_rx`
/// into the group. A `Raw` arriving during the wait is returned as
/// `RawPending` for the worker to handle as a fresh delivery.
pub struct PtyDeliveryWait<'a, 'b> {
    probe: &'b mut WorkerTerminalProbe<'a>,
    bytes_rx: &'b mut mpsc::Receiver<Vec<u8>>,
    write_rx: &'b mut mpsc::Receiver<super::transport::DeliveryCommand>,
    group: &'b mut Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
}

#[derive(Debug)]
pub enum WaitOutcome {
    /// A change was observed during the wait (the change atomic
    /// advanced). The state machine should re-observe.
    Changed,
    /// The deadline elapsed without a change. The state machine's
    /// prime-timeout branch will fire on the next classify step.
    TimedOut,
    /// A `Raw` command arrived during the wait (batch barrier).
    RawPending {
        content: String,
        append_enter: bool,
        outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
    },
}

impl<'a, 'b> PtyDeliveryWait<'a, 'b> {
    pub fn new(
        probe: &'b mut WorkerTerminalProbe<'a>,
        bytes_rx: &'b mut mpsc::Receiver<Vec<u8>>,
        write_rx: &'b mut mpsc::Receiver<super::transport::DeliveryCommand>,
        group: &'b mut Vec<(
            Box<DeliveryEnvelope>,
            oneshot::Sender<SingleDeliveryOutcome>,
        )>,
    ) -> Self {
        Self {
            probe,
            bytes_rx,
            write_rx,
            group,
        }
    }

    /// Run the wait loop until a change is observed, the deadline
    /// elapses, or a `Raw` command is absorbed from `write_rx`.
    pub fn run(&mut self, deadline: Instant) -> Result<WaitOutcome, DeliveryWaitError> {
        let atomic = self.probe.change_atomic();
        let initial = atomic.load(std::sync::atomic::Ordering::Acquire);
        while Instant::now() < deadline {
            // Drain bytes from the PTY reader thread. Each batch is
            // applied to the terminal and the change atomic is
            // advanced. This is the critical Finding 1 fix: without
            // this drain, the terminal would not reflect the latest
            // child output during the wait and the probe would never
            // observe a prompt-ready state.
            while let Ok(bytes) = self.bytes_rx.try_recv() {
                self.probe.terminal_mut().vt_write(&bytes);
                atomic.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            }
            // Drain envelopes from the write channel. `Envelope`
            // commands are absorbed into the current group (coalesce
            // during wait — Finding 3 fix). `Raw` is a batch barrier:
            // stop coalescing, return the raw to the caller for
            // processing as a fresh delivery.
            loop {
                match self.write_rx.try_recv() {
                    Ok(super::transport::DeliveryCommand::Envelope {
                        envelope,
                        outcome_tx,
                    }) => {
                        self.group.push((envelope, outcome_tx));
                    }
                    Ok(super::transport::DeliveryCommand::Raw {
                        content,
                        append_enter,
                        outcome_tx,
                    }) => {
                        return Ok(WaitOutcome::RawPending {
                            content,
                            append_enter,
                            outcome_tx,
                        });
                    }
                    Err(_) => break,
                }
            }
            // Check if a change was observed. If so, the state
            // machine's next classify step will see the fresh
            // terminal state.
            if atomic.load(std::sync::atomic::Ordering::Acquire) != initial {
                return Ok(WaitOutcome::Changed);
            }
            std::thread::sleep(WAIT_POLL_INTERVAL);
        }
        Ok(WaitOutcome::TimedOut)
    }
}

impl<'a> WorkerTerminalProbe<'a> {
    // Accessors `change_atomic` and `terminal_mut` are defined in
    // `state.rs` alongside the field declarations (private fields
    // are not accessible from this module).
}

/// Returned from [`DeliveryRun::start_envelope_group`] when the
/// initial write to the PTY master fails. The worker should discard
/// `failure_outcomes` (they were already sent to the group inside
/// `start_envelope_group`) and then, if `pending_raw` is `Some`,
/// start a new raw-only delivery for the raw.
pub struct DeliveryFailure {
    pub failure_outcomes_count: usize,
    pub pending_raw: Option<PendingRaw>,
}

impl DeliveryRun {
    /// Start a new envelope-group delivery. The group is the initial
    /// envelope; additional envelopes are absorbed from `write_rx`
    /// (BEFORE-WAIT coalescing). A `Raw` command during coalesce
    /// acts as a batch barrier: the run is built and the raw is
    /// stashed for the worker to take via `take_pending_raw` after
    /// the current group resolves. All envelopes are written to the
    /// PTY master immediately.
    ///
    /// Returns the new delivery. On write failure, returns
    /// `Err(DeliveryFailure { ... })`; the failure outcomes have
    /// already been sent to the group, so the worker just needs to
    /// process the pending raw (if any).
    #[allow(clippy::too_many_arguments)]
    pub fn start_envelope_group(
        first_envelope: Box<DeliveryEnvelope>,
        first_outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
        write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
        shared: &PtyShared,
        target_session: &str,
    ) -> Result<Self, DeliveryFailure> {
        let mut group: Vec<(
            Box<DeliveryEnvelope>,
            oneshot::Sender<SingleDeliveryOutcome>,
        )> = Vec::new();
        group.push((first_envelope, first_outcome_tx));
        let mut pending_raw: Option<PendingRaw> = None;
        loop {
            match write_rx.try_recv() {
                Ok(super::transport::DeliveryCommand::Envelope {
                    envelope,
                    outcome_tx,
                }) => group.push((envelope, outcome_tx)),
                Ok(super::transport::DeliveryCommand::Raw {
                    content,
                    append_enter,
                    outcome_tx,
                }) => {
                    pending_raw = Some(PendingRaw {
                        content,
                        append_enter,
                        outcome_tx,
                    });
                    break;
                }
                Err(_) => break,
            }
        }
        // Write all envelopes to the PTY master.
        for (envelope, _) in group.iter() {
            let text = envelope.message.render_pane_envelope(&envelope.message_id);
            let write_result = (|| -> std::io::Result<()> {
                let mut g = writer
                    .lock()
                    .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
                g.write_all(text.as_bytes())?;
                g.write_all(b"\n")?;
                Ok(())
            })();
            if let Err(e) = write_result {
                let reason = format!("pty master write: {e}");
                let outcomes = send_group_outcomes_with_explicit_failure(
                    &mut group,
                    reason,
                    "pty_write_failed",
                    target_session,
                );
                return Err(DeliveryFailure {
                    failure_outcomes_count: outcomes,
                    pending_raw,
                });
            }
        }
        let prime_started_at = Instant::now();
        let prime_timeout_ms = shared.config.prime_timeout_ms;
        let prime_deadline =
            prime_timeout_ms.map(|ms| prime_started_at + Duration::from_millis(ms));
        Ok(Self {
            group,
            quiet_window: QUIET_WINDOW,
            prime_started_at,
            prime_deadline,
            prime_timeout_ms,
            wedge_detection: shared.config.wedge_detection,
            qstate: QuiescenceState::new(),
            resolved: false,
        })
    }

    /// Drive the delivery state machine one step. The caller should
    /// re-service channels between calls. Outcomes for the current
    /// group are sent inside this method (each sender in the
    /// coalesced group is consumed with its per-message-id outcome).
    ///
    /// On a `Raw` batch barrier (Finding 3: `Raw` arriving during
    /// the wait), the run is resolved (current group sent) and the
    /// raw is stashed in `pending_raw`. The caller detects this by
    /// checking `take_pending_raw` after `step` returns `Done`.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
        write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        shared: &PtyShared,
        target_session: &str,
        pending_raw: &mut Option<PendingRaw>,
    ) -> DeliveryStep {
        let mut probe = WorkerTerminalProbe::new(
            terminal,
            shared.config.clone(),
            shared.last_change_atomic.clone(),
        );
        let classify = quiescence_classify_step(
            &mut probe,
            &mut self.qstate,
            target_session,
            self.quiet_window,
            self.prime_deadline,
            self.prime_started_at,
            self.prime_timeout_ms,
            self.wedge_detection,
        );
        match classify {
            QuiescenceAction::Done(result) => {
                self.resolved = true;
                send_group_outcomes(&mut self.group, result, target_session);
                DeliveryStep::Done
            }
            QuiescenceAction::NeedsWait(deadline) => {
                let wait_outcome = {
                    let mut wait =
                        PtyDeliveryWait::new(&mut probe, bytes_rx, write_rx, &mut self.group);
                    wait.run(deadline)
                };
                match wait_outcome {
                    Ok(WaitOutcome::Changed) => DeliveryStep::Continue,
                    Ok(WaitOutcome::TimedOut) => {
                        // The deadline elapsed without a change. Loop
                        // back to the classify step, which will check
                        // the prime_deadline and fire Timeout if
                        // applicable.
                        DeliveryStep::Continue
                    }
                    Ok(WaitOutcome::RawPending {
                        content,
                        append_enter,
                        outcome_tx,
                    }) => {
                        // Batch barrier: stash the raw and resolve
                        // the current group by running one more
                        // classify step. The classify sees the fresh
                        // terminal state (bytes were drained during
                        // the wait) and may resolve Delivered /
                        // Wedged / Timeout. If it still says
                        // NeedsWait, fall back to "raw interrupted"
                        // semantics.
                        *pending_raw = Some(PendingRaw {
                            content,
                            append_enter,
                            outcome_tx,
                        });
                        match quiescence_classify_step(
                            &mut probe,
                            &mut self.qstate,
                            target_session,
                            self.quiet_window,
                            self.prime_deadline,
                            self.prime_started_at,
                            self.prime_timeout_ms,
                            self.wedge_detection,
                        ) {
                            QuiescenceAction::Done(result) => {
                                self.resolved = true;
                                send_group_outcomes(&mut self.group, result, target_session);
                                DeliveryStep::Done
                            }
                            QuiescenceAction::NeedsWait(_) => {
                                self.resolved = true;
                                send_group_outcomes_with_explicit_failure(
                                    &mut self.group,
                                    "delivery interrupted by raw batch barrier".to_string(),
                                    "raw_interrupted",
                                    target_session,
                                );
                                DeliveryStep::Done
                            }
                        }
                    }
                    Err(e) => {
                        self.resolved = true;
                        send_group_outcomes(&mut self.group, Err(e), target_session);
                        DeliveryStep::Done
                    }
                }
            }
        }
    }

    /// Returns whether the delivery has resolved. The worker uses
    /// this to decide whether to drop the run and return to idle.
    pub fn is_resolved(&self) -> bool {
        self.resolved
    }
}

/// Process a raw-only delivery inline (no [`DeliveryRun`] involved).
/// Writes the raw content to the PTY master, then drives the same
/// state-machine wait as envelope-group deliveries (Finding 1: the
/// wait drains `bytes_rx` so the terminal reflects the latest child
/// output before each observation). The outcome is sent to the raw's
/// `outcome_tx` when the wait resolves.
///
/// The raw is a "lone" command — no prior envelope coalesce. The
/// worker calls this when:
/// - The relay submits a `Raw` directly (the worker's idle handler
///   picks it from `write_rx`).
/// - A `Raw` arrived during a prior envelope delivery's wait
///   (batch barrier); the worker takes it from the prior run's
///   `pending_raw` and processes it here.
///
/// `pending_outcome_slot` is set to `None` on entry (the caller
/// takes the prior `pending_raw` and passes the parts in) and to
/// `Some(PendingRaw)` if a second raw arrives during this raw's
/// wait (cascade — the worker should process the second raw next).
#[allow(clippy::too_many_arguments)]
pub fn deliver_raw(
    raw: PendingRaw,
    terminal: &mut libghostty_vt::Terminal<'static, 'static>,
    bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
    write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
    writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    shared: &PtyShared,
    target_session: &str,
    pending_outcome_slot: &mut Option<PendingRaw>,
) {
    let PendingRaw {
        content,
        append_enter,
        outcome_tx,
    } = raw;
    let write_result = (|| -> std::io::Result<()> {
        let mut g = writer
            .lock()
            .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
        g.write_all(content.as_bytes())?;
        if append_enter {
            g.write_all(b"\n")?;
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = outcome_tx.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_write_failed".to_string()),
            reason: Some(format!("pty master write: {e}")),
            details: None,
        });
        return;
    }
    let mut qstate = QuiescenceState::new();
    let prime_started_at = Instant::now();
    let prime_timeout_ms = shared.config.prime_timeout_ms;
    let prime_deadline = prime_timeout_ms.map(|ms| prime_started_at + Duration::from_millis(ms));
    let wedge_detection = shared.config.wedge_detection;
    let mut empty_group: Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )> = Vec::new();
    loop {
        let mut probe = WorkerTerminalProbe::new(
            terminal,
            shared.config.clone(),
            shared.last_change_atomic.clone(),
        );
        let classify = quiescence_classify_step(
            &mut probe,
            &mut qstate,
            target_session,
            QUIET_WINDOW,
            prime_deadline,
            prime_started_at,
            prime_timeout_ms,
            wedge_detection,
        );
        match classify {
            QuiescenceAction::Done(result) => {
                let outcome = envelope_outcome_from_wait_result(result, target_session);
                let _ = outcome_tx.send(outcome);
                return;
            }
            QuiescenceAction::NeedsWait(deadline) => {
                let wait_outcome = {
                    let mut wait =
                        PtyDeliveryWait::new(&mut probe, bytes_rx, write_rx, &mut empty_group);
                    wait.run(deadline)
                };
                match wait_outcome {
                    Ok(WaitOutcome::Changed) => continue,
                    Ok(WaitOutcome::TimedOut) => continue,
                    Ok(WaitOutcome::RawPending {
                        content,
                        append_enter,
                        outcome_tx,
                    }) => {
                        // Cascade: a second raw arrived during this
                        // raw's wait. Stash it for the worker's next
                        // iteration. The first raw's wait continues.
                        *pending_outcome_slot = Some(PendingRaw {
                            content,
                            append_enter,
                            outcome_tx,
                        });
                    }
                    Err(e) => {
                        let outcome = envelope_outcome_from_wait_result(Err(e), target_session);
                        let _ = outcome_tx.send(outcome);
                        return;
                    }
                }
            }
        }
    }
}

/// Send one [`SingleDeliveryOutcome`] per envelope in the coalesced
/// group, derived from the wait result. Each sender keeps its own
/// `message_id`. Returns the number of outcomes sent.
fn send_group_outcomes(
    group: &mut Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    wait_result: Result<String, DeliveryWaitError>,
    target_session: &str,
) -> usize {
    let base = envelope_outcome_from_wait_result(wait_result, target_session);
    let count = group.len();
    for (env, sender) in group.drain(..) {
        let mut outcome = base.clone();
        outcome.message_id = env.message_id;
        let _ = sender.send(outcome);
    }
    count
}

/// Send one failure outcome per envelope in the group. Returns the
/// number of outcomes sent.
fn send_group_outcomes_with_explicit_failure(
    group: &mut Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    reason: String,
    reason_code: &str,
    target_session: &str,
) -> usize {
    let count = group.len();
    for (env, sender) in group.drain(..) {
        let _ = sender.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: env.message_id,
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some(reason_code.to_string()),
            reason: Some(reason.clone()),
            details: None,
        });
    }
    count
}

fn envelope_outcome_from_wait_result(
    wait_result: Result<String, DeliveryWaitError>,
    target_session: &str,
) -> SingleDeliveryOutcome {
    match wait_result {
        Ok(_pane_target) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Delivered,
            reason_code: None,
            reason: None,
            details: None,
        },
        Err(DeliveryWaitError::Timeout {
            timeout,
            readiness_mismatch,
            mismatch_reason,
        }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Timeout,
            reason_code: Some("delivery_prime_timeout".to_string()),
            reason: Some(format!(
                "prime wait timed out after {}ms (readiness_mismatch={}, reason={:?})",
                timeout.as_millis(),
                readiness_mismatch,
                mismatch_reason
            )),
            details: None,
        },
        Err(DeliveryWaitError::Wedged { reason }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pane_wedged".to_string()),
            reason: Some(format!("pty pane wedged: {reason}")),
            details: None,
        },
        Err(DeliveryWaitError::Failed { reason }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_probe_failed".to_string()),
            reason: Some(reason),
            details: None,
        },
        Err(DeliveryWaitError::Shutdown) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::DroppedOnShutdown,
            reason_code: Some("dropped_on_shutdown".to_string()),
            reason: Some("delivery dropped due to relay shutdown".to_string()),
            details: None,
        },
    }
}
