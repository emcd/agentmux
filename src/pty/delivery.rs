//! Pty worker delivery and write-result handling.
//!
//! The Pty worker thread is the only thread that can apply bytes to
//! the libghostty-vt terminal (the terminal is `!Send + !Sync`).
//! Handover readiness is observed by `PtyTransport::can_accept_handover`;
//! once a command reaches this worker, its master write is the submission
//! evidence and no prompt or quiescence wait is performed.

use std::{io::Write, sync::Arc};

use tokio::sync::{mpsc, oneshot};

use crate::transports::{
    DeliveryDiagnosticContext, DeliveryEnvelope, DeliveryWaitError, SingleDeliveryOutcome,
};

use super::state::PtyShared;

/// Marker line written immediately before a terminal-outcome receipt's
/// pane envelope so the receiving agent can distinguish a relay/system
/// status update from a peer message at a glance.
const RECEIPT_MARKER: &str = "--- agentmux terminal-outcome receipt ---";

/// A `Raw` command waiting to be processed by the next delivery.
/// Held on the worker at the normal mail/raw FIFO barrier.
pub struct PendingRaw {
    pub content: String,
    pub append_enter: bool,
    pub outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
}

/// Returned from [`DeliveryRun::start_envelope_group`] when the
/// initial write to the PTY master fails. The failure outcomes have
/// already been sent to the group inside `start_envelope_group`.
/// The worker should honor `pending_raw` (if `Some`) by starting a
/// new raw-only delivery for the raw.
pub struct DeliveryFailure {
    pub pending_raw: Option<PendingRaw>,
}

/// Marker for "the write failed and the failure outcome was already
/// sent" — used by `Delivery::start_raw` to signal failure without
/// returning a unit `Result`.
#[derive(Debug)]
pub struct RawWriteFailed;

/// One in-flight delivery on the worker thread. Owns the coalesced
/// envelope group until each master-write outcome is resolved.
pub struct DeliveryRun {
    group: Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    resolved: bool,
}

/// Outcome of one [`Delivery::step`] call. The worker loop uses
/// this to drive its event loop. Outcomes for the current group are
/// sent inside `step` (each sender in the coalesced group is
/// consumed with its per-message-id outcome); the variant tells the
/// worker whether the run is done or still in progress.
#[derive(Debug, PartialEq, Eq)]
pub enum DeliveryStep {
    /// The delivery is still in progress. The worker should
    /// re-service channels and call `step` again.
    Continue,
    /// The delivery has resolved. The current group's outcomes have
    /// been sent to each sender. The worker should drop the run and
    /// return to idle. `wedged` is `true` when the resolution was a
    /// `DeliveryWaitError::Wedged { .. }`; the worker uses that flag
    /// to publish `WorkerReadinessState::Unavailable` instead of
    /// `Available` on wedge-class resolutions (per the worker-
    /// readiness contract; see `src/pty/transport.rs` `run_worker`'s
    /// `Done { wedged }` branch).
    Done { wedged: bool },
}

/// One in-flight raw-only delivery. The raw write has already reached
/// the master when this value enters the worker step loop.
pub struct RawDelivery {
    raw: Option<PendingRaw>,
    resolved: bool,
}

/// One in-flight delivery on the worker thread. Either an envelope
/// group or a raw-only command. The worker loop holds an
/// `Option<Delivery>` and calls `step` per outer iteration.
pub enum Delivery {
    Group(DeliveryRun),
    Raw(RawDelivery),
}

impl Delivery {
    /// Start a new envelope-group delivery. Absorbs additional
    /// `Envelope` commands from `write_rx` (BEFORE-WAIT
    /// coalescing). A `Raw` command during coalesce acts as a
    /// batch barrier: the run is built and the raw is stashed
    /// for the worker to take after the current group resolves.
    /// All envelopes are written to the PTY master immediately.
    ///
    /// Returns the new delivery. On write failure, returns
    /// `Err(DeliveryFailure { pending_raw })`; the failure
    /// outcomes have already been sent to the group, so the
    /// worker just needs to process the pending raw (if any).
    #[allow(clippy::too_many_arguments)]
    pub fn start_envelope_group(
        first_envelope: Box<DeliveryEnvelope>,
        first_outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
        write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
        _shared: &PtyShared,
        target_session: &str,
    ) -> Result<Delivery, DeliveryFailure> {
        let mut run = DeliveryRun::new();
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
        // Write all envelopes to the PTY master. Receipt envelopes get a
        // leading marker line so the receiving agent can distinguish a
        // terminal-outcome receipt from a peer message at a glance. The
        // marker and the envelope are written under the same writer lock
        // so the two cannot be interleaved with another write on the
        // same Pty master.
        for (envelope, _) in group.iter() {
            let text = envelope.message.render_pane_envelope(&envelope.message_id);
            let write_result = (|| -> std::io::Result<()> {
                let mut g = writer
                    .lock()
                    .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
                if envelope.is_receipt {
                    g.write_all(RECEIPT_MARKER.as_bytes())?;
                    g.write_all(b"\n")?;
                }
                g.write_all(text.as_bytes())?;
                g.write_all(b"\n")?;
                Ok(())
            })();
            if let Err(e) = write_result {
                let reason = format!("pty master write: {e}");
                send_group_outcomes_with_explicit_failure(
                    &mut group,
                    reason,
                    "pty_write_failed",
                    target_session,
                );
                return Err(DeliveryFailure { pending_raw });
            }
        }
        run.group = group;
        Ok(Delivery::Group(run))
    }

    /// Start a raw-only delivery. Writes the raw content to the
    /// PTY master and constructs a `RawDelivery` ready for the
    /// worker's step loop. On write failure, returns
    /// `Err(RawWriteFailed)` (the failure outcome has already been
    /// sent to the raw's `outcome_tx`).
    #[allow(clippy::too_many_arguments)]
    pub fn start_raw(
        content: String,
        append_enter: bool,
        outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
        writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
        _shared: &PtyShared,
        target_session: &str,
    ) -> Result<Delivery, RawWriteFailed> {
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
            return Err(RawWriteFailed);
        }
        let raw = PendingRaw {
            content,
            append_enter,
            outcome_tx,
        };
        let raw_delivery = RawDelivery::new(raw);
        Ok(Delivery::Raw(raw_delivery))
    }

    /// Drive the delivery state machine one step. The worker
    /// should re-service channels (notably `snapshot_rx`)
    /// between calls. The wait is decomposed into per-tick polls
    /// so the worker event loop stays responsive to look
    /// requests during long waits.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
        write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        shared: &PtyShared,
        diagnostics: &DeliveryDiagnosticContext<'_>,
        pending_raw: &mut Option<PendingRaw>,
    ) -> DeliveryStep {
        match self {
            Delivery::Group(run) => run.step(
                terminal,
                bytes_rx,
                write_rx,
                shared,
                diagnostics,
                pending_raw,
            ),
            Delivery::Raw(raw) => raw.step(
                terminal,
                bytes_rx,
                write_rx,
                shared,
                diagnostics,
                pending_raw,
            ),
        }
    }

    /// Returns whether the delivery has resolved. The worker uses
    /// this to decide whether to drop the run and return to idle.
    pub fn is_resolved(&self) -> bool {
        match self {
            Delivery::Group(run) => run.resolved,
            Delivery::Raw(raw) => raw.resolved,
        }
    }

    /// Abandon an in-flight delivery by resolving it as `Failed`
    /// with the supplied `reason_code` and `reason`. Called from the
    /// child-exit branch of the worker event loop so the caller's
    /// `OutcomeFuture` does not hang after the worker has observed
    /// child death and refused to drive the delivery to its terminal
    /// classification.
    ///
    /// For an envelope-group delivery, every sender in the
    /// coalesced group receives the same `Failed` outcome. For a
    /// raw-only delivery, the raw's `outcome_tx` is resolved.
    /// Best-effort: a `send` failure (the caller has dropped their
    /// receiver) is silently swallowed.
    pub fn abandon_into_failed(&mut self, target_session: &str, reason_code: &str, reason: &str) {
        match self {
            Delivery::Group(run) => {
                send_group_outcomes_with_explicit_failure(
                    &mut run.group,
                    reason.to_string(),
                    reason_code,
                    target_session,
                );
                run.resolved = true;
            }
            Delivery::Raw(raw) => {
                if let Some(r) = raw.raw.take() {
                    let _ = r.outcome_tx.send(SingleDeliveryOutcome {
                        target_session: target_session.to_string(),
                        message_id: String::new(),
                        outcome: crate::transports::SendOutcome::Failed,
                        reason_code: Some(reason_code.to_string()),
                        reason: Some(reason.to_string()),
                        details: None,
                    });
                }
                raw.resolved = true;
            }
        }
    }
}

impl DeliveryRun {
    fn new() -> Self {
        Self {
            group: Vec::new(),
            resolved: false,
        }
    }

    /// Drive the delivery state machine one step. When the run is
    /// in a wait, do one wait poll. Otherwise, do one classify.
    /// On `NeedsWait`, set up the wait and return `Continue`; the
    /// next step will do the first wait poll.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
        write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        shared: &PtyShared,
        diagnostics: &DeliveryDiagnosticContext<'_>,
        _pending_raw: &mut Option<PendingRaw>,
    ) -> DeliveryStep {
        let _ = (bytes_rx, write_rx, _pending_raw);
        self.step_classify(terminal, shared, diagnostics)
    }

    fn step_classify(
        &mut self,
        _terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        _shared: &PtyShared,
        diagnostics: &DeliveryDiagnosticContext<'_>,
    ) -> DeliveryStep {
        self.resolved = true;
        send_group_outcomes(
            &mut self.group,
            Ok(String::new()),
            diagnostics.target_session,
        );
        DeliveryStep::Done { wedged: false }
    }
}

impl RawDelivery {
    fn new(raw: PendingRaw) -> Self {
        Self {
            raw: Some(raw),
            resolved: false,
        }
    }

    fn step(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
        _write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        shared: &PtyShared,
        diagnostics: &DeliveryDiagnosticContext<'_>,
        _pending_raw: &mut Option<PendingRaw>,
    ) -> DeliveryStep {
        let _ = (bytes_rx, _write_rx, _pending_raw);
        self.step_classify(terminal, shared, diagnostics)
    }

    fn step_classify(
        &mut self,
        _terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        _shared: &PtyShared,
        diagnostics: &DeliveryDiagnosticContext<'_>,
    ) -> DeliveryStep {
        self.resolved = true;
        if let Some(raw) = self.raw.take() {
            let _ = raw.outcome_tx.send(envelope_outcome_from_wait_result(
                Ok(String::new()),
                diagnostics.target_session,
            ));
        }
        DeliveryStep::Done { wedged: false }
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
        // Pty builds its bounds with no readiness bound, so the shared
        // classifier cannot reach its readiness-expiry path for a Pty group.
        // Mapped rather than asserted for the same reason the Tmux transport
        // maps its unreachable `Wedged` arm: this runs on the worker thread
        // behind a tokio task, where a panic is isolated and swallowed, leaving
        // the sender with no outcome at all. A structured failure is more
        // observable than an assertion nothing can hear. It is deliberately not
        // mapped to `Timeout`, which would present a bound Pty does not have as
        // though it had elapsed.
        Err(DeliveryWaitError::ReadinessTimeout { reason_code, .. }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_probe_failed".to_string()),
            reason: Some(format!(
                "pty classifier returned a readiness expiry it cannot produce \
                 (pty carries no readiness bound): {}",
                reason_code.code()
            )),
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
