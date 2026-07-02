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
//! [`DeliveryRun`] (envelope-group delivery) and [`RawDelivery`]
//! (raw-only delivery) wrap the cross-transport state machine
//! ([`quiescence_classify_step`]) and drive it one step at a time.
//! Between steps the worker event loop services other channels
//! (snapshot requests, PTY byte drain, write_rx absorption). The
//! step's wait helper ([`PtyWait::one_poll`]) drains `bytes_rx` so
//! the terminal reflects the latest child output before the next
//! observation. Envelope-group waits also absorb additional
//! `Envelope` commands from `write_rx` into the current group
//! (coalesce-during-wait); raw waits do NOT absorb envelopes
//! (they would be dropped, since the raw has no group to write
//! them to). A `Raw` command arriving during an envelope-group wait
//! is returned as a `RawPending` signal so the worker can resolve
//! the current group and process the raw as a fresh delivery
//! (batch-barrier semantics).

use std::{
    io::Write,
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant},
};

use tokio::sync::{mpsc, oneshot};

use crate::transports::{
    DeliveryEnvelope, DeliveryWaitError, QuiescenceAction, QuiescenceState, SingleDeliveryOutcome,
    quiescence_classify_step,
};

use super::state::{PtyShared, WorkerTerminalProbe};

const QUIET_WINDOW: Duration = Duration::from_millis(50);
/// Per-tick sleep when the wait is in progress. Bounds the wait-poll
/// frequency so the outer loop can service other channels (snapshot
/// requests) between polls.
pub const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A `Raw` command waiting to be processed by the next delivery.
/// Held on the worker between iterations when a batch barrier was
/// hit (either during coalesce-before-wait or during the wait).
pub struct PendingRaw {
    pub content: String,
    pub append_enter: bool,
    pub outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
}

/// Distinguishes the two kinds of Pty-specific waits. Envelope-group
/// waits absorb additional `Envelope` commands from `write_rx` into
/// the current group (coalesce-during-wait) and treat `Raw` as a
/// batch barrier. Raw-only waits do NOT drain `write_rx` at all —
/// envelopes stay queued for the next outer-loop iteration, and raw
/// commands are picked up the same way (avoids the v1 bug where
/// raw waits absorbed envelopes into a throwaway group and dropped
/// them on the floor when `deliver_raw` returned).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKind {
    EnvelopeGroup,
    RawOnly,
}

/// Per-wait state preserved across [`PtyWait::one_poll`] calls.
/// `initial_atomic` is the value of the change atomic at the start
/// of the wait; the wait returns `Changed` when the atomic advances
/// past it.
pub struct PtyWait {
    kind: WaitKind,
    initial_atomic: u64,
    deadline: Instant,
}

/// Outcome of one [`PtyWait::one_poll`] call. `Continue` means the
/// wait is still in progress; the caller should re-service other
/// channels and call `one_poll` again.
#[derive(Debug)]
pub enum WaitOutcome {
    /// The wait is still in progress. The caller should re-service
    /// other channels (e.g. snapshot requests) and call `one_poll`
    /// again.
    Continue,
    /// A change was observed during this poll (the change atomic
    /// advanced). The state machine should re-observe.
    Changed,
    /// The deadline elapsed without a change during this poll. The
    /// state machine's prime-timeout branch will fire on the next
    /// classify step.
    TimedOut,
    /// A `Raw` command arrived during this poll (batch barrier).
    /// Only returned by `WaitKind::EnvelopeGroup` waits (raw waits
    /// don't drain `write_rx`).
    RawPending {
        content: String,
        append_enter: bool,
        outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
    },
}

impl PtyWait {
    /// Begin a new wait. The caller provides the deadline (from
    /// `prime_deadline` or the unbounded fallback) and the change
    /// atomic the probe tracks.
    pub fn new(kind: WaitKind, atomic: &AtomicU64, deadline: Instant) -> Self {
        Self {
            kind,
            initial_atomic: atomic.load(std::sync::atomic::Ordering::Acquire),
            deadline,
        }
    }

    /// Run one tick of the wait: drain `bytes_rx` (apply to
    /// terminal, advance the change atomic), drain `write_rx` if
    /// the wait kind allows (envelope-group only), then check
    /// whether the change atomic advanced or the deadline elapsed.
    ///
    /// Does NOT sleep. Returns `Continue` when no terminal outcome
    /// was reached during this tick; the outer worker loop can
    /// service other channels (snapshot requests) before the next
    /// call.
    pub fn one_poll(
        &self,
        probe: &mut WorkerTerminalProbe<'_>,
        bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
        write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        group: &mut Vec<(
            Box<DeliveryEnvelope>,
            oneshot::Sender<SingleDeliveryOutcome>,
        )>,
    ) -> WaitOutcome {
        // Drain bytes from the PTY reader thread. Each batch is
        // applied to the terminal and the change atomic is
        // advanced. This is the critical "child output reaches the
        // terminal during wait" behavior: without this drain, the
        // probe would never observe a prompt-ready state during
        // the wait and the wait would fire Timeout / Wedged.
        let atomic = probe.change_atomic();
        while let Ok(bytes) = bytes_rx.try_recv() {
            probe.terminal_mut().vt_write(&bytes);
            atomic.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        // Drain envelopes / detect raw from the write channel.
        // Behavior depends on the wait kind:
        //
        // - EnvelopeGroup: absorb `Envelope` commands into the
        //   current group (coalesce-during-wait). `Raw` is a
        //   batch barrier: return RawPending so the worker can
        //   resolve the current group and process the raw as a
        //   fresh delivery.
        // - RawOnly: do NOT drain `write_rx` at all. Envelopes
        //   stay queued for the worker's next outer-loop
        //   iteration; raw commands are picked up the same way.
        //   This avoids the v1 bug where raw waits absorbed
        //   envelopes into a throwaway group and dropped them.
        match self.kind {
            WaitKind::EnvelopeGroup => loop {
                match write_rx.try_recv() {
                    Ok(super::transport::DeliveryCommand::Envelope {
                        envelope,
                        outcome_tx,
                    }) => {
                        group.push((envelope, outcome_tx));
                    }
                    Ok(super::transport::DeliveryCommand::Raw {
                        content,
                        append_enter,
                        outcome_tx,
                    }) => {
                        return WaitOutcome::RawPending {
                            content,
                            append_enter,
                            outcome_tx,
                        };
                    }
                    Err(_) => break,
                }
            },
            WaitKind::RawOnly => {
                // Intentionally do not drain write_rx. See comment
                // above.
            }
        }
        // Check if a change was observed. If so, the state
        // machine's next classify step will see the fresh
        // terminal state.
        if atomic.load(std::sync::atomic::Ordering::Acquire) != self.initial_atomic {
            return WaitOutcome::Changed;
        }
        // Check the deadline.
        if Instant::now() >= self.deadline {
            return WaitOutcome::TimedOut;
        }
        WaitOutcome::Continue
    }
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
/// envelope group and the per-delivery state-machine state. Drives
/// the cross-transport state machine one step at a time via
/// [`DeliveryRun::step`].
pub struct DeliveryRun {
    group: Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    quiet_window: Duration,
    prime_started_at: Instant,
    prime_deadline: Option<Instant>,
    prime_timeout_ms: Option<u64>,
    wedge_detection: bool,
    qstate: QuiescenceState,
    /// Active wait. `Some` when the state machine is between
    /// classify steps (NeedsWait was returned); the run does one
    /// wait poll per `step` call.
    wait: Option<PtyWait>,
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
    /// return to idle.
    Done,
}

/// One in-flight raw-only delivery. Writes the raw content to the
/// PTY master, then drives the same state-machine wait as
/// envelope-group deliveries. The wait is steppable: the worker
/// calls [`RawDelivery::step`] once per outer iteration, and the
/// wait's `one_poll` drains `bytes_rx` without touching `write_rx`
/// (so envelopes queued behind the raw are not dropped).
pub struct RawDelivery {
    raw: Option<PendingRaw>,
    quiet_window: Duration,
    prime_started_at: Instant,
    prime_deadline: Option<Instant>,
    prime_timeout_ms: Option<u64>,
    wedge_detection: bool,
    qstate: QuiescenceState,
    wait: Option<PtyWait>,
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
        shared: &PtyShared,
        target_session: &str,
    ) -> Result<Delivery, DeliveryFailure> {
        let mut run = DeliveryRun::new(shared);
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
        shared: &PtyShared,
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
        let raw_delivery = RawDelivery::new(shared, raw);
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
        target_session: &str,
        pending_raw: &mut Option<PendingRaw>,
    ) -> DeliveryStep {
        match self {
            Delivery::Group(run) => run.step(
                terminal,
                bytes_rx,
                write_rx,
                shared,
                target_session,
                pending_raw,
            ),
            Delivery::Raw(raw) => raw.step(
                terminal,
                bytes_rx,
                write_rx,
                shared,
                target_session,
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
}

impl DeliveryRun {
    fn new(shared: &PtyShared) -> Self {
        let prime_started_at = Instant::now();
        let prime_timeout_ms = shared.config.prime_timeout_ms;
        let prime_deadline =
            prime_timeout_ms.map(|ms| prime_started_at + Duration::from_millis(ms));
        Self {
            group: Vec::new(),
            quiet_window: QUIET_WINDOW,
            prime_started_at,
            prime_deadline,
            prime_timeout_ms,
            wedge_detection: shared.config.wedge_detection,
            qstate: QuiescenceState::new(),
            wait: None,
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
        target_session: &str,
        _pending_raw: &mut Option<PendingRaw>,
    ) -> DeliveryStep {
        if self.wait.is_some() {
            self.step_wait_poll(
                terminal,
                bytes_rx,
                write_rx,
                shared,
                target_session,
                _pending_raw,
            )
        } else {
            self.step_classify(terminal, shared, target_session)
        }
    }

    fn step_classify(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        shared: &PtyShared,
        target_session: &str,
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
                // Initialize wait state; the next step will do the
                // first wait poll.
                self.wait = Some(PtyWait::new(
                    WaitKind::EnvelopeGroup,
                    &shared.last_change_atomic,
                    deadline,
                ));
                DeliveryStep::Continue
            }
        }
    }

    fn step_wait_poll(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
        write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        shared: &PtyShared,
        target_session: &str,
        pending_raw: &mut Option<PendingRaw>,
    ) -> DeliveryStep {
        // Borrow the wait (and probe) for the duration of this
        // poll. PtyWait::one_poll is the only place that touches
        // bytes_rx / write_rx / group during the wait, so the
        // borrows don't outlive the call.
        let wait = self
            .wait
            .as_ref()
            .expect("step_wait_poll called without wait");
        let mut probe = WorkerTerminalProbe::new(
            terminal,
            shared.config.clone(),
            shared.last_change_atomic.clone(),
        );
        let outcome = wait.one_poll(&mut probe, bytes_rx, write_rx, &mut self.group);
        match outcome {
            WaitOutcome::Changed => {
                self.wait = None;
                DeliveryStep::Continue
            }
            WaitOutcome::TimedOut => {
                self.wait = None;
                DeliveryStep::Continue
            }
            WaitOutcome::RawPending {
                content,
                append_enter,
                outcome_tx,
            } => {
                // Batch barrier: stash the raw and resolve the
                // current group by running one more classify
                // (which sees the fresh terminal state). If the
                // classify still wants to wait, fall back to
                // "raw interrupted" semantics.
                *pending_raw = Some(PendingRaw {
                    content,
                    append_enter,
                    outcome_tx,
                });
                self.wait = None;
                // Drop the probe borrow before the recursive call.
                drop(probe);
                match self.classify_after_raw_barrier(terminal, shared, target_session) {
                    Some(action) => action,
                    None => {
                        // Classify still wants to wait. Treat the
                        // current group as interrupted by the raw.
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
            WaitOutcome::Continue => DeliveryStep::Continue,
        }
    }

    /// One more classify after a raw batch barrier. Returns
    /// `Some(DeliveryStep)` if the classify resolved (Done) or
    /// `None` if the classify still wants to wait (the caller
    /// treats the group as interrupted).
    fn classify_after_raw_barrier(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        shared: &PtyShared,
        target_session: &str,
    ) -> Option<DeliveryStep> {
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
                Some(DeliveryStep::Done)
            }
            QuiescenceAction::NeedsWait(_) => None,
        }
    }
}

impl RawDelivery {
    fn new(shared: &PtyShared, raw: PendingRaw) -> Self {
        let prime_started_at = Instant::now();
        let prime_timeout_ms = shared.config.prime_timeout_ms;
        let prime_deadline =
            prime_timeout_ms.map(|ms| prime_started_at + Duration::from_millis(ms));
        Self {
            raw: Some(raw),
            quiet_window: QUIET_WINDOW,
            prime_started_at,
            prime_deadline,
            prime_timeout_ms,
            wedge_detection: shared.config.wedge_detection,
            qstate: QuiescenceState::new(),
            wait: None,
            resolved: false,
        }
    }

    fn step(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
        _write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        shared: &PtyShared,
        target_session: &str,
        _pending_raw: &mut Option<PendingRaw>,
    ) -> DeliveryStep {
        if self.wait.is_some() {
            self.step_wait_poll(terminal, bytes_rx, _write_rx, shared, target_session)
        } else {
            self.step_classify(terminal, shared, target_session)
        }
    }

    fn step_classify(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        shared: &PtyShared,
        target_session: &str,
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
                let outcome = envelope_outcome_from_wait_result(result, target_session);
                if let Some(raw) = self.raw.take() {
                    let _ = raw.outcome_tx.send(outcome);
                }
                DeliveryStep::Done
            }
            QuiescenceAction::NeedsWait(deadline) => {
                // Initialize wait state; the next step will do
                // the first wait poll. WaitKind::RawOnly so the
                // wait does NOT drain write_rx (envelopes stay
                // queued for the next outer-loop iteration; raw
                // commands are picked up the same way).
                self.wait = Some(PtyWait::new(
                    WaitKind::RawOnly,
                    &shared.last_change_atomic,
                    deadline,
                ));
                DeliveryStep::Continue
            }
        }
    }

    fn step_wait_poll(
        &mut self,
        terminal: &mut libghostty_vt::Terminal<'static, 'static>,
        bytes_rx: &mut mpsc::Receiver<Vec<u8>>,
        write_rx: &mut mpsc::Receiver<super::transport::DeliveryCommand>,
        shared: &PtyShared,
        _target_session: &str,
    ) -> DeliveryStep {
        let wait = self
            .wait
            .as_ref()
            .expect("step_wait_poll called without wait");
        let mut probe = WorkerTerminalProbe::new(
            terminal,
            shared.config.clone(),
            shared.last_change_atomic.clone(),
        );
        // Raw-only wait: the wait's one_poll will NOT touch
        // write_rx or group (kind=RawOnly). We still pass
        // write_rx + a throwaway empty group to satisfy the
        // one_poll signature; the wait ignores them. This
        // avoids the v1 bug where raw waits absorbed envelopes
        // into a throwaway group and dropped them on the floor.
        let mut empty_group: Vec<(
            Box<DeliveryEnvelope>,
            oneshot::Sender<SingleDeliveryOutcome>,
        )> = Vec::new();
        let outcome = wait.one_poll(&mut probe, bytes_rx, write_rx, &mut empty_group);
        match outcome {
            WaitOutcome::Changed => {
                self.wait = None;
                DeliveryStep::Continue
            }
            WaitOutcome::TimedOut => {
                self.wait = None;
                DeliveryStep::Continue
            }
            WaitOutcome::RawPending { .. } => {
                // WaitKind::RawOnly should never return
                // RawPending (it doesn't drain write_rx).
                // Defensive: treat as TimedOut.
                self.wait = None;
                DeliveryStep::Continue
            }
            WaitOutcome::Continue => DeliveryStep::Continue,
        }
    }
}

impl WorkerTerminalProbe<'_> {
    // Accessors `change_atomic` and `terminal_mut` are defined in
    // `state.rs` alongside the field declarations (private fields
    // are not accessible from this module).
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
