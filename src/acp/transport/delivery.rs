//! ACP's half of the delivery-loop executor: what it observes, what it combines
//! into one turn, and what submitting that turn proves.
//!
//! The loop itself is [`run_delivery_executor`](crate::transports::run_delivery_executor)
//! and is shared with every other transport. What lives here is the three
//! decisions that are genuinely ACP's — whether the worker can start a turn now,
//! how much of a peeked run fits one `session/prompt` under the token budget, and
//! what dispatching that prompt proved.
//!
//! **The write is the delivery boundary and the turn is not.** A framed
//! `session/prompt` that succeeds resolves every member of its unit at the write,
//! before the replay-buffer locks or the dispatch handler run. What follows —
//! the turn completing, a permission request, a connection closing — is
//! target-health observability that drives readiness and the respawn signal, and
//! never a second outcome for an already-resolved member.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use crate::acp::client::AcpStdioClient;
use crate::envelope::{PromptBatchSettings, batch_envelope_groups};
use crate::protocol::mailbox::{MailboxEntry, MailboxPayload};
use crate::transports::{
    DeliveryWriter, PeekDimensions, PlannedWrite, SubmissionEvidence, TransportHealth,
    UnreachableSince, WorkerReadinessState, receipt_runs,
};

use super::state::AcpSharedState;
use super::turn::{TurnContext, TurnObservation, submit_turn};

/// The live agent connection one turn is submitted through.
///
/// Held as an `Option` on the writer rather than by value, because this executor
/// outlives any particular one of them: it is spawned when the transport first
/// prepares for startup and runs until the transport stops, across a bootstrap
/// that never succeeded and across every respawn.
pub(crate) struct AcpRuntimeSlot {
    pub(crate) client: AcpStdioClient,
    pub(crate) session_id: String,
}

/// What the transport hands its executor when a runtime arrives or departs.
pub(crate) enum RuntimeInstall {
    /// A bootstrap or respawn established a connection; write through this one
    /// from now on.
    Install(Box<AcpRuntimeSlot>),
    /// The runtime was released ahead of a respawn. The executor keeps running
    /// and keeps reporting, but writes nothing until one is installed again.
    Clear,
}

/// What the driver knows about this target's reachability and the executor does
/// not.
///
/// `Unavailable` readiness is published for a respawn gap and for a permanent
/// give-up alike, so the executor cannot tell them apart from readiness. Only the
/// driver's abandonment latch separates "no runtime right now" from "no runtime
/// ever again", and only the second may start a dwell.
pub struct AcpReachability {
    pub(crate) abandoned: Arc<AtomicBool>,
    pub(crate) unreachable_since: Arc<UnreachableSince>,
}

impl AcpReachability {
    /// Binds an executor's view of reachability to the latches its driver folds.
    ///
    /// Both are shared rather than copied: the driver latches abandonment when a
    /// respawn gives up, and the dwell is measured from the instant the same
    /// latch first recorded. A second latch here would restart that instant on
    /// every poll, and a dwell measured from a moving `since` never elapses.
    #[must_use]
    pub fn new(abandoned: Arc<AtomicBool>, unreachable_since: Arc<UnreachableSince>) -> Self {
        Self {
            abandoned,
            unreachable_since,
        }
    }
}

/// Channels connecting the transport to its delivery-loop executor.
pub(crate) struct DeliveryChannels {
    /// Runtimes installed and released over this executor's life.
    pub(crate) runtime_rx: Receiver<RuntimeInstall>,
    /// Latched by `fence_generation` and `shutdown`. Deliberately **not** by
    /// `release_runtime`: a respawn replaces this transport's runtime, not its
    /// executor, and an executor that stopped for one would leave the target's
    /// mailbox unconsumed across every respawn gap.
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) respawn_needed_tx: tokio::sync::watch::Sender<u64>,
}

pub(crate) struct DeliveryTaskIdentity {
    pub(crate) target_session: String,
}

/// What one iteration decided to submit as a turn.
pub(crate) struct AcpTurn {
    prompt: String,
    /// The sessions authorized to decide any permission request this turn
    /// raises, taken from the head envelope of the unit.
    decider_sessions: Vec<String>,
    /// The head member's id, which correlates a permission request back to the
    /// send that provoked it.
    head_message_id: String,
}

/// The ACP transport's contribution to the shared delivery loop.
///
/// **One of these per transport instance, not per runtime.** It is spawned when
/// the transport first prepares for startup and runs until the transport is
/// fenced or shut down; a respawn hands it a replacement connection rather than
/// starting a second executor beside it. That is what makes the seriality the
/// spec requires structural: there is only ever one thread issuing framed writes
/// for this target, so the order it writes in is the order the agent sees.
///
/// It also has to exist when no runtime does. A target whose bootstrap failed
/// permanently is unreachable, and unreachability is carried by the dwell — which
/// only a running executor observes. An executor that existed only alongside a
/// live client would leave exactly that target's entries queued forever.
pub(crate) struct AcpDeliveryWriter {
    /// The live connection, absent before the first bootstrap succeeds, across a
    /// respawn gap, and forever after a permanent failure.
    runtime: Option<AcpRuntimeSlot>,
    runtime_rx: Receiver<RuntimeInstall>,
    shared: Arc<AcpSharedState>,
    chooser: Option<crate::transports::Chooser>,
    target_session: String,
    batch_settings: PromptBatchSettings,
    respawn_needed_tx: tokio::sync::watch::Sender<u64>,
    reachability: AcpReachability,
    stop: Arc<AtomicBool>,
    /// A dispatched turn this executor has yet to wait out.
    ///
    /// Held rather than observed inside the write, so a member's evidence is
    /// settled before anything waits on the agent. The wait happens in
    /// [`is_ready`](DeliveryWriter::is_ready), which is where it belongs: the
    /// turn completing is precisely what makes the worker ready for the next
    /// one, and the executor asks that question before every write anyway.
    outstanding_turn: Option<TurnObservation>,
}

impl AcpDeliveryWriter {
    pub(crate) fn new(
        channels: DeliveryChannels,
        shared: Arc<AcpSharedState>,
        chooser: Option<crate::transports::Chooser>,
        batch_settings: PromptBatchSettings,
        identity: DeliveryTaskIdentity,
        reachability: AcpReachability,
    ) -> Self {
        Self {
            runtime: None,
            runtime_rx: channels.runtime_rx,
            shared,
            chooser,
            target_session: identity.target_session,
            batch_settings,
            respawn_needed_tx: channels.respawn_needed_tx,
            reachability,
            stop: channels.stop,
            outstanding_turn: None,
        }
    }

    /// Applies every runtime installed or released since the last pass.
    ///
    /// Drained rather than read once, so a bootstrap and an immediate respawn
    /// leave the newest connection installed rather than the executor writing
    /// through a superseded one. Dropping a replaced client here is what closes
    /// the connection it held.
    fn apply_runtime_changes(&mut self) {
        loop {
            match self.runtime_rx.try_recv() {
                Ok(RuntimeInstall::Install(slot)) => {
                    // A replacement arriving with a turn still outstanding
                    // belongs to the connection that is going away; observing it
                    // against the new client would attribute one agent's answer
                    // to another.
                    self.outstanding_turn = None;
                    self.runtime = Some(*slot);
                }
                Ok(RuntimeInstall::Clear) => {
                    self.outstanding_turn = None;
                    self.runtime = None;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    fn is_available(&self) -> bool {
        matches!(
            *self.shared.readiness.lock().expect("readiness mutex"),
            WorkerReadinessState::Available
        )
    }
}

impl DeliveryWriter for AcpDeliveryWriter {
    type Plan = AcpTurn;

    fn peek_dimensions(&self) -> PeekDimensions {
        PeekDimensions::for_session_type(crate::configuration::SessionType::Acp)
            .expect("acp declares peek dimensions")
    }

    fn health(&self) -> TransportHealth {
        // Readiness cannot answer this: `Unavailable` is published for a respawn
        // gap and for a permanent give-up alike, so reading it would start a
        // dwell against a target a respawn was about to make deliverable. The
        // driver's abandonment latch is the one thing that separates them, which
        // is why it is shared into this executor rather than left where only a
        // relay-facing `health` call could see it — under the pull model nothing
        // calls that, so a target abandoned mid-bootstrap would otherwise have
        // its unreachability observed by nobody at all.
        self.reachability
            .unreachable_since
            .fold(!self.reachability.abandoned.load(Ordering::Acquire))
    }

    fn is_ready(&mut self) -> bool {
        self.apply_runtime_changes();
        // The turn this executor dispatched last is waited out here, not at the
        // write that dispatched it. Its members are already resolved either way;
        // what the wait settles is when this worker becomes ready again, which is
        // exactly the question being asked.
        //
        // Both are taken together or not at all: an observation only means
        // anything against the client that produced it, and
        // `apply_runtime_changes` has already dropped any observation whose
        // connection went away.
        if let Some(runtime) = self.runtime.as_mut()
            && let Some(observation) = self.outstanding_turn.take()
        {
            // Borrows are split by hand because the observation needs the client
            // mutably while the context borrows the fields beside it.
            let context = TurnContext {
                session_id: runtime.session_id.as_str(),
                shared: &self.shared,
                chooser: &self.chooser,
                target_session: self.target_session.as_str(),
            };
            observation.observe(&mut runtime.client, &context, &self.respawn_needed_tx);
        }
        // No runtime, no write — and no claim about the target either. This is
        // the respawn gap and the pre-bootstrap window, both of which leave
        // entries queued and undeclared exactly as an unready agent does.
        if self.runtime.is_none() {
            return false;
        }
        // Only `Available` qualifies. A `Busy` worker is mid-turn and cannot take
        // another; the wider "runtime exists" reading that `Busy` also satisfies
        // is what the mirrored registry state carries for its other observers.
        self.is_available()
    }

    fn plan(&mut self, entries: &[MailboxEntry]) -> Option<PlannedWrite<Self::Plan>> {
        let head = entries.first()?;
        if let MailboxPayload::Raw { content, .. } = &head.payload {
            // Raw is submitted through the same `session/prompt` path as mail —
            // ACP has no second channel to write it through — but alone, and
            // without envelope framing. `no_enter` has no meaning on a wire
            // protocol with no terminal to submit to.
            return Some(PlannedWrite {
                entry_count: 1,
                rendered: AcpTurn {
                    prompt: content.clone(),
                    decider_sessions: Vec::new(),
                    head_message_id: head.message_id.clone(),
                },
            });
        }

        let envelopes: Vec<_> = entries
            .iter()
            .filter_map(|entry| match &entry.payload {
                MailboxPayload::Mail(envelope) => Some(envelope),
                MailboxPayload::Raw { .. } => None,
            })
            .collect();
        let head_envelope = envelopes.first()?;
        let rendered: Vec<String> = envelopes
            .iter()
            .map(|envelope| envelope.message.render_pane_envelope(&envelope.message_id))
            .collect();
        // A receipt never rides with peer traffic; see `receipt_runs`.
        let first_run = *receipt_runs(
            &envelopes
                .iter()
                .map(|envelope| envelope.is_receipt)
                .collect::<Vec<_>>(),
        )
        .first()?;
        // The first budget group of the first run. One unit at a time, because
        // the relay accepts one declared-and-unacked unit per target: a plan
        // spanning two groups could be declared neither as one nor as two.
        let group = batch_envelope_groups(&rendered[..first_run], self.batch_settings)
            .into_iter()
            .next()?;
        Some(PlannedWrite {
            entry_count: group.member_count,
            rendered: AcpTurn {
                prompt: group.combined_prompt,
                // Taken from the head, as the correlation always was: a
                // permission request belongs to the turn, and the turn is named
                // by the member that opened it.
                decider_sessions: head_envelope.choice_decider_sessions.clone(),
                head_message_id: head_envelope.message_id.clone(),
            },
        })
    }

    fn write(&mut self, planned: PlannedWrite<Self::Plan>) -> Vec<SubmissionEvidence> {
        let Some(runtime) = self.runtime.as_mut() else {
            // Unreachable on the loop's own path — `is_ready` refuses without a
            // runtime — but a declared unit must be acknowledged whatever
            // happens, and `NotSubmitted` is provable here: there is no
            // connection to have written through.
            return vec![SubmissionEvidence::NotSubmitted; planned.entry_count];
        };
        let (evidence, observation) = {
            let context = TurnContext {
                session_id: runtime.session_id.as_str(),
                shared: &self.shared,
                chooser: &self.chooser,
                target_session: self.target_session.as_str(),
            };
            submit_turn(
                &mut runtime.client,
                &context,
                &self.respawn_needed_tx,
                planned.rendered.prompt.as_str(),
                planned.rendered.head_message_id.as_str(),
                planned.rendered.decider_sessions.as_slice(),
            )
        };
        self.outstanding_turn = observation;
        // One framed write carries the whole unit, so one result answers for
        // every member of it. That is the property the packing unit exists to
        // record: they shared a submission, so they share its fate.
        vec![evidence; planned.entry_count]
    }

    fn stop_requested(&mut self) -> bool {
        // Raised by `fence_generation` and `shutdown`, and deliberately not by
        // `release_runtime`. A respawn replaces this transport's runtime, not its
        // executor: stopping here would end the only consumer the target's
        // mailbox has, and the replacement would have to start a second executor
        // beside a first that may not have returned yet — which is the seriality
        // violation this shape exists to make unrepresentable.
        self.stop.load(Ordering::Acquire)
    }

    fn wait_for_work(&mut self, doorbell: &crate::protocol::DeliveryDoorbell, timeout: Duration) {
        // A runtime installed while this executor was idle is picked up before
        // the wait rather than after it, so a bootstrap completing does not have
        // to wait out a poll interval before its first write.
        self.apply_runtime_changes();
        doorbell.wait_for(timeout);
    }
}
