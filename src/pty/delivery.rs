//! Pty worker delivery and write-result handling.
//!
//! Readiness is checked before authorization. Once a command reaches this
//! worker, the PTY master write is the submission boundary and its outcome is
//! resolved immediately; no prompt, prime, or quiescence wait follows it.

use std::{io::Write, sync::Arc};

use tokio::sync::{mpsc, oneshot};

use crate::transports::{DeliveryEnvelope, SendOutcome, SingleDeliveryOutcome};

use super::transport::DeliveryCommand;

const RECEIPT_MARKER: &str = "--- agentmux terminal-outcome receipt ---";

pub struct PendingRaw {
    pub content: String,
    pub append_enter: bool,
    pub outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
}

pub struct DeliveryFailure {
    pub pending_raw: Option<PendingRaw>,
}

#[derive(Debug)]
pub struct RawWriteFailed;

pub struct DeliveryRun {
    group: Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    resolved: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeliveryStep {
    Done,
}

pub struct RawDelivery {
    raw: Option<PendingRaw>,
    resolved: bool,
}

pub enum Delivery {
    Group(DeliveryRun),
    Raw(RawDelivery),
}

impl Delivery {
    #[allow(clippy::too_many_arguments)]
    pub fn start_envelope_group(
        first_envelope: Box<DeliveryEnvelope>,
        first_outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
        write_rx: &mut mpsc::Receiver<DeliveryCommand>,
        writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
        target_session: &str,
    ) -> Result<Delivery, DeliveryFailure> {
        let mut group = vec![(first_envelope, first_outcome_tx)];
        let mut pending_raw = None;
        loop {
            match write_rx.try_recv() {
                Ok(DeliveryCommand::Envelope {
                    envelope,
                    outcome_tx,
                }) => group.push((envelope, outcome_tx)),
                Ok(DeliveryCommand::Raw {
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

        for (envelope, _) in &group {
            let text = envelope.message.render_pane_envelope(&envelope.message_id);
            let result = (|| -> std::io::Result<()> {
                let mut guard = writer
                    .lock()
                    .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
                if envelope.is_receipt {
                    guard.write_all(RECEIPT_MARKER.as_bytes())?;
                    guard.write_all(b"\n")?;
                }
                guard.write_all(text.as_bytes())?;
                guard.write_all(b"\n")
            })();
            if let Err(error) = result {
                send_group_failure(
                    &mut group,
                    target_session,
                    "pty_write_failed",
                    &error.to_string(),
                );
                return Err(DeliveryFailure { pending_raw });
            }
        }

        Ok(Delivery::Group(DeliveryRun {
            group,
            resolved: false,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_raw(
        content: String,
        append_enter: bool,
        outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
        writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
        target_session: &str,
    ) -> Result<Delivery, RawWriteFailed> {
        let result = (|| -> std::io::Result<()> {
            let mut guard = writer
                .lock()
                .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
            guard.write_all(content.as_bytes())?;
            if append_enter {
                guard.write_all(b"\n")?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = outcome_tx.send(failed_outcome(
                target_session,
                "pty_write_failed",
                &error.to_string(),
            ));
            return Err(RawWriteFailed);
        }
        Ok(Delivery::Raw(RawDelivery {
            raw: Some(PendingRaw {
                content,
                append_enter,
                outcome_tx,
            }),
            resolved: false,
        }))
    }

    pub fn step(&mut self, target_session: &str) -> DeliveryStep {
        match self {
            Delivery::Group(run) => run.resolve(target_session),
            Delivery::Raw(raw) => raw.resolve(target_session),
        }
    }

    pub fn abandon_into_failed(&mut self, target_session: &str, reason_code: &str, reason: &str) {
        match self {
            Delivery::Group(run) => {
                send_group_failure(&mut run.group, target_session, reason_code, reason);
                run.resolved = true;
            }
            Delivery::Raw(raw) => {
                if let Some(item) = raw.raw.take() {
                    let _ =
                        item.outcome_tx
                            .send(failed_outcome(target_session, reason_code, reason));
                }
                raw.resolved = true;
            }
        }
    }
}

impl DeliveryRun {
    fn resolve(&mut self, target_session: &str) -> DeliveryStep {
        self.resolved = true;
        for (envelope, sender) in self.group.drain(..) {
            let _ = sender.send(delivered_outcome(target_session, &envelope.message_id));
        }
        DeliveryStep::Done
    }
}

impl RawDelivery {
    fn resolve(&mut self, target_session: &str) -> DeliveryStep {
        self.resolved = true;
        if let Some(raw) = self.raw.take() {
            let _ = raw.outcome_tx.send(delivered_outcome(target_session, ""));
        }
        DeliveryStep::Done
    }
}

fn delivered_outcome(target_session: &str, message_id: &str) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session: target_session.to_string(),
        message_id: message_id.to_string(),
        outcome: SendOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

fn failed_outcome(target_session: &str, reason_code: &str, reason: &str) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session: target_session.to_string(),
        message_id: String::new(),
        outcome: SendOutcome::Failed,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.to_string()),
        details: None,
    }
}

fn send_group_failure(
    group: &mut Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    target_session: &str,
    reason_code: &str,
    reason: &str,
) {
    for (envelope, sender) in group.drain(..) {
        let mut outcome = failed_outcome(target_session, reason_code, reason);
        outcome.message_id = envelope.message_id;
        let _ = sender.send(outcome);
    }
}
