//! Tmux's half of the delivery-loop executor: what it observes, what it packs,
//! and what it pastes.
//!
//! The loop itself is [`run_delivery_executor`](crate::transports::run_delivery_executor)
//! and is shared with every other transport. What lives here is the three
//! decisions that are genuinely tmux's — whether the pane will take a write now,
//! how much of a peeked run fits one paste, and what pasting it proves — plus
//! nothing else, because everything else is the same on four transports and is
//! stated once where they share it.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::envelope::{PromptBatchSettings, batch_envelope_groups};
use crate::protocol::mailbox::{MailboxEntry, MailboxPayload};
use crate::transports::{
    DeliveryWriter, PeekDimensions, PlannedWrite, SubmissionEvidence, TransportHealth,
    UnreachableSince,
};

use super::PaneObservation;
use super::delivery::coalescing_runs;
use crate::tmux::pane::{inject_literal_text, resolve_active_pane_target};

/// What one iteration decided to paste.
///
/// Rendered during planning rather than at the write, so the declaration the
/// relay records sits between the decision and its effect rather than inside it.
pub(super) enum TmuxWrite {
    /// A token-budget-bounded prompt carrying one or more mail entries.
    Prompt(String),
    /// Raw input, written through verbatim.
    Raw { content: String, append_enter: bool },
}

/// The tmux transport's contribution to the shared delivery loop.
pub(super) struct TmuxDeliveryWriter {
    socket: PathBuf,
    target_session: String,
    batch_settings: PromptBatchSettings,
    /// The observer thread's most recent reading, shared rather than re-observed.
    ///
    /// The executor must not spawn a tmux client of its own to answer a readiness
    /// question: the observer already runs on an owned thread that publishes into
    /// a fence-reachable invocation slot, and a second observer would be a second
    /// client per poll for a reading that already exists.
    observation: Arc<Mutex<PaneObservation>>,
    /// Shared with the transport, so the level the executor acts on and the level
    /// the transport reports cannot disagree about when unreachability began.
    unreachable_since: Arc<UnreachableSince>,
    stop: Arc<AtomicBool>,
}

impl TmuxDeliveryWriter {
    pub(super) fn new(
        socket: PathBuf,
        target_session: String,
        batch_settings: PromptBatchSettings,
        observation: Arc<Mutex<PaneObservation>>,
        unreachable_since: Arc<UnreachableSince>,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            socket,
            target_session,
            batch_settings,
            observation,
            unreachable_since,
            stop,
        }
    }

    fn observed(&self) -> PaneObservation {
        *self.observation.lock().expect("tmux observation mutex")
    }

    /// Resolves the pane every write goes through.
    ///
    /// Its failure is the one write failure tmux can report as provable
    /// non-delivery: nothing has been injected when it fires, so no byte can have
    /// reached the pane.
    fn pane(&self) -> Option<String> {
        resolve_active_pane_target(self.socket.as_path(), self.target_session.as_str()).ok()
    }
}

impl DeliveryWriter for TmuxDeliveryWriter {
    type Plan = TmuxWrite;

    fn peek_dimensions(&self) -> PeekDimensions {
        PeekDimensions::for_session_type(crate::configuration::SessionType::Tmux)
            .expect("tmux declares peek dimensions")
    }

    fn health(&self) -> TransportHealth {
        // `Pending` reports healthy. Not having observed yet is ignorance, not
        // evidence, and starting a dwell against a pane nobody has examined would
        // resolve members on the strength of not knowing. Such a target reports
        // unready through the readiness axis instead, so its entries are held.
        self.unreachable_since
            .fold(!matches!(self.observed(), PaneObservation::Unreachable))
    }

    fn activity_generation(&self) -> u64 {
        self.observed().activity()
    }

    fn is_ready(&mut self) -> bool {
        matches!(
            self.observed(),
            PaneObservation::Observed { ready: true, .. }
        )
    }

    fn plan(&mut self, entries: &[MailboxEntry]) -> Option<PlannedWrite<Self::Plan>> {
        let head = entries.first()?;
        if let MailboxPayload::Raw {
            content,
            append_enter,
        } = &head.payload
        {
            // A raw entry is peeked alone and written alone. It carries no
            // exemption from the declare-before-write discipline, only from
            // coalescing.
            return Some(PlannedWrite {
                entry_count: 1,
                rendered: TmuxWrite::Raw {
                    content: content.clone(),
                    append_enter: *append_enter,
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
        let rendered: Vec<String> = envelopes
            .iter()
            .map(|envelope| super::render_paste_text(envelope))
            .collect();
        // Split at receipt boundaries before budgeting, so no prompt ever mixes a
        // terminal-outcome receipt with peer traffic. That separation predates
        // this loop and survives it for a reason of its own: a receipt reads as a
        // relay/system notice to the receiving agent, and burying one inside a
        // peer's message is what made it indistinguishable.
        let first_run = *coalescing_runs(
            &envelopes
                .iter()
                .map(|envelope| envelope.is_receipt)
                .collect::<Vec<_>>(),
        )
        .first()?;
        // The first budget group of the first run. One unit at a time is not a
        // simplification: the relay accepts one declared-and-unacked unit per
        // target, so a plan covering two groups could not be declared as one and
        // could not be declared as two either.
        let group = batch_envelope_groups(&rendered[..first_run], self.batch_settings)
            .into_iter()
            .next()?;
        Some(PlannedWrite {
            entry_count: group.member_count,
            rendered: TmuxWrite::Prompt(group.combined_prompt),
        })
    }

    fn write(&mut self, planned: PlannedWrite<Self::Plan>) -> Vec<SubmissionEvidence> {
        let members = planned.entry_count;
        let outcome = |evidence| vec![evidence; members];
        let Some(pane) = self.pane() else {
            // Provable non-delivery: the pane could not be resolved, so nothing
            // was injected.
            return outcome(SubmissionEvidence::NotSubmitted);
        };
        let injected = match &planned.rendered {
            // Envelope-mode writes always submit with Enter; the combined prompt
            // is pasted once for the whole unit.
            TmuxWrite::Prompt(prompt) => {
                inject_literal_text(self.socket.as_path(), pane.as_str(), prompt.as_str(), true)
            }
            TmuxWrite::Raw {
                content,
                append_enter,
            } => inject_literal_text(
                self.socket.as_path(),
                pane.as_str(),
                content.as_str(),
                *append_enter,
            ),
        };
        match injected {
            Ok(()) => outcome(SubmissionEvidence::Submitted),
            // A paste is a body write followed by an Enter, so a failure cannot
            // exclude partial effect. `NotSubmitted` would be a positive claim
            // this transport cannot support once the injection has begun.
            Err(_) => outcome(SubmissionEvidence::SubmissionUnknown),
        }
    }

    fn stop_requested(&mut self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

/// Builds the writer for a target's pane on `runtime_directory`'s tmux socket.
pub(super) fn tmux_delivery_writer(
    runtime_directory: &Path,
    target_session: String,
    batch_settings: PromptBatchSettings,
    observation: Arc<Mutex<PaneObservation>>,
    unreachable_since: Arc<UnreachableSince>,
    stop: Arc<AtomicBool>,
) -> TmuxDeliveryWriter {
    TmuxDeliveryWriter::new(
        crate::runtime::paths::tmux_socket_path_for_runtime_directory(runtime_directory),
        target_session,
        batch_settings,
        observation,
        unreachable_since,
        stop,
    )
}
