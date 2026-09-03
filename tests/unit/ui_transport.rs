//! Unit coverage for the first-class UI transport.
//!
//! Exercises the broadcast its delivery executor performs (via the injected
//! services closures), what an absent subscriber proves, and the unsupported
//! raw-write / non-lookable capability surface. Nothing is handed to the
//! transport: entries are seeded into a stub mailbox and the transport's own
//! executor peeks them, which is the shape production has.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use agentmux::envelope::AddressIdentity;
use agentmux::protocol::DeliveryDoorbell;
use agentmux::protocol::mailbox::{EntrySequence, MailboxEntry, MailboxPayload};
use agentmux::transports::{
    DeliveryEnvelope, DeliveryExecutorContext, DeliveryMessage, GenerationFence, StartupContext,
    SubmissionEvidence, Transport, UiBroadcastStatus, UiIncomingMessage, UiTransport,
    UiTransportServices,
};

use crate::stub_mailbox::StubMailbox;

/// A transport whose executor consumes `mailbox`, started and ready to run.
///
/// `startup` is where the executor is spawned, so every test here goes through
/// it: a transport that was never started has no executor, and asserting against
/// one would assert against a mailbox nothing was reading.
fn started_transport(services: UiTransportServices, mailbox: &Arc<StubMailbox>) -> UiTransport {
    let mut transport = UiTransport::new(
        services,
        DeliveryExecutorContext {
            consumer: Arc::clone(mailbox) as Arc<_>,
            doorbell: DeliveryDoorbell::default(),
            poll_interval: Duration::from_millis(5),
            unreachable_dwell: Duration::from_secs(30),
        },
    );
    transport
        .startup(StartupContext {
            namespace: "party".to_string(),
            runtime_directory: std::path::PathBuf::from("/nonexistent/ui"),
            target_member: agentmux::configuration::BundleMember {
                id: "bob".to_string(),
                name: None,
                working_directory: None,
                target: agentmux::configuration::TargetConfiguration::Ui,
                coder_session_id: None,
                policy_id: None,
                environment: Vec::new(),
            },
            choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
                decided_by: String::new(),
                reason_code: "not_applicable".to_string(),
                reason: None,
            }),
        })
        .expect("ui transport startup");
    transport
}

/// One mail entry at the head of a UI target's mailbox.
fn mail_entry(envelope: DeliveryEnvelope) -> MailboxEntry {
    MailboxEntry {
        sequence: EntrySequence::first(),
        message_id: envelope.message_id.clone(),
        canonical_bytes: envelope.message.body.len() as u64,
        payload: MailboxPayload::Mail(Arc::new(envelope)),
    }
}

/// Spins until the mailbox has an acknowledgment to report.
fn wait_for_ack(mailbox: &StubMailbox) -> agentmux::transports::SubmissionEvidence {
    let started = Instant::now();
    loop {
        if let Some(acked) = mailbox.acked().first() {
            return acked.evidence;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the executor acknowledged nothing within the bound",
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn ui_envelope() -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: "m-1".to_string(),
        message: DeliveryMessage {
            body: "hello ui".to_string(),
            created_at: "2026-03-05T00:00:00Z".to_string(),
            namespace: "party".to_string(),
            sender: AddressIdentity {
                session_name: "alice@party".to_string(),
                display_name: Some("Alice".to_string()),
            },
            target: AddressIdentity {
                session_name: "bob@party".to_string(),
                display_name: Some("Bob".to_string()),
            },
            cc: vec![AddressIdentity {
                session_name: "carol@party".to_string(),
                display_name: None,
            }],
            authenticated_identity: Some("principal-alice".to_string()),
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        is_receipt: false,
    }
}

/// Spins until `flag` is set, failing rather than hanging if it never is.
fn wait_for(flag: &AtomicBool, description: &str) {
    let started = Instant::now();
    while !flag.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for {description}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn ui_executor_broadcasts_incoming_and_acknowledges_submitted() {
    let captured: Arc<Mutex<Option<UiIncomingMessage>>> = Arc::new(Mutex::new(None));
    let phases: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let services = UiTransportServices {
        broadcast_incoming: {
            let captured = captured.clone();
            Arc::new(move |incoming: &UiIncomingMessage| {
                *captured.lock().unwrap() = Some(incoming.clone());
                UiBroadcastStatus::Delivered
            })
        },
        emit_phase: {
            let phases = phases.clone();
            Arc::new(move |phase| {
                phases.lock().unwrap().push(phase.phase.to_string());
                UiBroadcastStatus::Delivered
            })
        },
    };

    let mailbox = Arc::new(StubMailbox::with_entries(vec![mail_entry(ui_envelope())]));
    let mut transport = started_transport(services, &mailbox);

    assert_eq!(wait_for_ack(&mailbox), SubmissionEvidence::Submitted);
    assert_eq!(mailbox.acked()[0].message_id, "m-1");
    transport.fence_generation();

    let incoming = captured.lock().unwrap().clone().expect("incoming captured");
    assert_eq!(incoming.message_id, "m-1");
    assert_eq!(incoming.sender_session, "alice@party");
    assert_eq!(incoming.body, "hello ui");
    assert_eq!(incoming.cc_sessions, vec!["carol@party".to_string()]);
    assert_eq!(
        incoming.authenticated_identity.as_deref(),
        Some("principal-alice"),
    );

    // The routed probe precedes the broadcast; the delivered phase mirrors success.
    let phases = phases.lock().unwrap().clone();
    assert_eq!(phases, vec!["routed".to_string(), "delivered".to_string()]);
}

/// A delivery with no UI listening acknowledges `NotSubmitted` at once, from one
/// attempt rather than a wait.
///
/// This replaces a bounded reconnect poll. The wait was an absence timer with a
/// budget only this transport knew, and elapsed time decided the outcome. What
/// it bought — a message still queued when a UI happens to come back — is worth
/// little while nothing replays to a reconnecting UI, and it cost every send to
/// an unwatched target thirty seconds before the sender heard anything.
///
/// `NotSubmitted` rather than an unknown: no subscriber received the broadcast,
/// which the transport observed rather than inferred.
#[test]
fn a_broadcast_with_no_endpoint_acknowledges_not_submitted() {
    let phases: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let services = UiTransportServices {
        broadcast_incoming: Arc::new(|_incoming: &UiIncomingMessage| UiBroadcastStatus::NoUi),
        emit_phase: {
            let phases = phases.clone();
            Arc::new(move |phase| {
                phases.lock().unwrap().push(phase.phase.to_string());
                UiBroadcastStatus::NoUi
            })
        },
    };

    let mailbox = Arc::new(StubMailbox::with_entries(vec![mail_entry(ui_envelope())]));
    let mut transport = started_transport(services, &mailbox);

    assert_eq!(wait_for_ack(&mailbox), SubmissionEvidence::NotSubmitted);
    transport.fence_generation();

    // One attempt, not a poll. A `routed` phase that reported no endpoint used
    // to send the executor back around a sleep loop; the absence of a second
    // one is what shows the wait is gone rather than merely shortened. Read
    // after the fence, so the executor cannot add a phase between the two.
    assert_eq!(
        phases.lock().unwrap().clone(),
        vec!["routed".to_string()],
        "the delivery attempts once and reports what that attempt proved"
    );
}

/// The UI generation's destructive fence step is revocation, and revocation is
/// what makes the fence's second observation able to succeed.
///
/// UI owns no child, so `terminate_generation` has no process to signal. The
/// tempting reading of that is that the step has nothing to do — and if it did
/// nothing, a delivery already past the cooperative flag would go on to reach a
/// subscriber after its generation had been terminated, which is the exact
/// overlap the fence exists to prevent. So what is under test is that the step
/// still has an effect without a child behind it.
///
/// The executor is parked inside the `routed` phase when the fence begins, which
/// is what makes the two steps distinguishable: `is_fenced` is read once at the
/// start of the delivery and this executor is already past it, so the cooperative
/// request cannot reach it. Only the revocation check immediately before the
/// broadcast can stop it.
///
/// Both halves are driven, and that contrast is what carries the test. The
/// acknowledgment cannot say *which* check stopped the write — it reports
/// evidence and nothing else — so a run that only fenced is what shows the
/// cooperative flag genuinely cannot reach an executor already past it. Without
/// it, a `NotSubmitted` produced by the fenced rung would pass while proving
/// nothing about the destructive step.
///
/// The steps are driven directly rather than through `acknowledge_fence`, because
/// the protocol's windows and verdicts are covered against a controllable
/// generation in `unit/delivery_fence.rs`; what is wanted here is the real
/// transport's response to each step, without racing a timer to deliver them.
///
/// Not asserted: the absence of a child process, because no UI code path spawns
/// one and there is nothing to observe the absence of. The testable content of
/// "no owned child" is that the destructive step still bites, which is what the
/// broadcast count and the evidence pin.
#[test]
fn a_terminated_ui_generation_stops_emitting_without_a_child_to_signal() {
    /// Runs one delivery parked in the `routed` phase, applies `steps` while it
    /// is parked, and reports what the write proved and how many broadcasts it
    /// emitted.
    fn parked_delivery(terminate: bool) -> (SubmissionEvidence, usize, bool) {
        let probe_entered = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        let broadcasts = Arc::new(AtomicUsize::new(0));

        let services = UiTransportServices {
            broadcast_incoming: {
                let broadcasts = broadcasts.clone();
                Arc::new(move |_incoming: &UiIncomingMessage| {
                    broadcasts.fetch_add(1, Ordering::SeqCst);
                    UiBroadcastStatus::Delivered
                })
            },
            emit_phase: {
                let probe_entered = probe_entered.clone();
                let released = released.clone();
                Arc::new(move |phase| {
                    // Park the executor inside the probe, where neither fence
                    // flag has been read yet on this iteration.
                    if phase.phase == "routed" {
                        probe_entered.store(true, Ordering::SeqCst);
                        while !released.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    UiBroadcastStatus::Delivered
                })
            },
        };

        let mailbox = Arc::new(StubMailbox::with_entries(vec![mail_entry(ui_envelope())]));
        let mut transport = started_transport(services, &mailbox);
        wait_for(
            &probe_entered,
            "the delivery executor to enter the routed probe",
        );

        // An executor of this generation is running, so it has not ceased.
        // Reading cessation as true here is the dangerous answer: it would let a
        // replacement generation start alongside a live writer.
        let ceased_while_running = transport.generation_ceased();

        transport.fence_generation();
        if terminate {
            transport.terminate_generation();
        }
        released.store(true, Ordering::SeqCst);

        let evidence = wait_for_ack(&mailbox);

        // The observation the fence's second window would make. Polled rather
        // than read once: the executor acknowledges before it returns, so the
        // handle can still be live for an instant after the evidence lands.
        let started = Instant::now();
        while !transport.generation_ceased() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "a fenced generation whose executor has acknowledged must cease",
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        (
            evidence,
            broadcasts.load(Ordering::SeqCst),
            ceased_while_running,
        )
    }

    // Cooperative step only. The executor is already past the flag's one read,
    // so it goes on to broadcast — which is exactly why the destructive step
    // exists, and what makes the contrast below evidence rather than assertion.
    let (evidence, broadcasts, ceased_while_running) = parked_delivery(false);
    assert!(
        !ceased_while_running,
        "a generation with a running executor must not report cessation",
    );
    assert_eq!(
        (evidence, broadcasts),
        (SubmissionEvidence::Submitted, 1),
        "the cooperative flag cannot reach an executor already past its read",
    );

    // Both steps. Same parked position, same release, and now nothing is
    // emitted: only the revocation check immediately before the broadcast can
    // have stopped it.
    let (evidence, broadcasts, ceased_while_running) = parked_delivery(true);
    assert!(
        !ceased_while_running,
        "a generation with a running executor must not report cessation",
    );
    assert_eq!(
        (evidence, broadcasts),
        (SubmissionEvidence::NotSubmitted, 0),
        "a terminated generation must not emit to a subscriber",
    );
}

/// A raw entry reaching a UI executor is written by nothing and acknowledged
/// `NotSubmitted`, rather than left at the head of the mailbox.
///
/// The relay's `raww` capability gate rejects a non-raw-writable target at the
/// request boundary, so no raw entry is ever admitted for a UI target and this
/// arm is unreachable in production. It is covered anyway because what it
/// protects against is not a raw write succeeding — it is a raw entry parking
/// every message behind it for the life of the target, which is what an arm that
/// declined to plan would do.
///
/// `NotSubmitted` and not an unknown: the arm emits no frame at all, so
/// non-delivery is provable rather than inferred.
#[test]
fn a_raw_entry_a_ui_cannot_write_is_acknowledged_rather_than_parked() {
    let broadcasts = Arc::new(AtomicUsize::new(0));
    let services = UiTransportServices {
        broadcast_incoming: {
            let broadcasts = broadcasts.clone();
            Arc::new(move |_| {
                broadcasts.fetch_add(1, Ordering::SeqCst);
                UiBroadcastStatus::Delivered
            })
        },
        emit_phase: Arc::new(|_| UiBroadcastStatus::Delivered),
    };

    let mailbox = Arc::new(StubMailbox::with_entries(vec![MailboxEntry {
        sequence: EntrySequence::first(),
        message_id: "m-raw".to_string(),
        canonical_bytes: 8,
        payload: MailboxPayload::Raw {
            content: "raw text".to_string(),
            append_enter: true,
        },
    }]));
    let mut transport = started_transport(services, &mailbox);

    assert_eq!(wait_for_ack(&mailbox), SubmissionEvidence::NotSubmitted);
    transport.fence_generation();
    assert!(
        mailbox.is_drained(),
        "an unwritable entry must leave the mailbox rather than park what follows it",
    );
    assert_eq!(
        broadcasts.load(Ordering::SeqCst),
        0,
        "the unsupported arm must emit no frame",
    );
}

#[test]
fn ui_transport_is_not_lookable() {
    let services = UiTransportServices {
        broadcast_incoming: Arc::new(|_| UiBroadcastStatus::Delivered),
        emit_phase: Arc::new(|_| UiBroadcastStatus::Delivered),
    };
    let mailbox = Arc::new(StubMailbox::empty());
    let transport = started_transport(services, &mailbox);
    assert!(transport.give_output().is_none());
}

/// Parity guard for the relay/97 delivery-event contract: the `incoming_message`
/// event MUST carry the bare canonical `session@namespace` id for
/// `sender_session`/`cc_sessions` via the non-decorating
/// `AddressIdentity::canonical_session_id()` accessor — never the decorating
/// `render_address` pane-header form (`Display Name <session:session_name>`).
///
/// The fixture deliberately sets `display_name` distinct from `session_name` on
/// BOTH the sender and the cc party, so this test bites if anyone later
/// "upgrades" the event-build path to `render_address`: the decorated form would
/// differ from the asserted bare id and fail here.
#[test]
fn ui_incoming_message_emits_bare_canonical_identity_never_decorated() {
    let captured: Arc<Mutex<Option<UiIncomingMessage>>> = Arc::new(Mutex::new(None));

    let services = UiTransportServices {
        broadcast_incoming: {
            let captured = captured.clone();
            Arc::new(move |incoming: &UiIncomingMessage| {
                *captured.lock().unwrap() = Some(incoming.clone());
                UiBroadcastStatus::Delivered
            })
        },
        emit_phase: Arc::new(|_phase| UiBroadcastStatus::Delivered),
    };

    let envelope = DeliveryEnvelope {
        message_id: "m-parity".to_string(),
        message: DeliveryMessage {
            body: "parity body".to_string(),
            created_at: "2026-03-05T00:00:00Z".to_string(),
            namespace: "bundle".to_string(),
            sender: AddressIdentity {
                session_name: "alice@bundle".to_string(),
                display_name: Some("Alice Cooper".to_string()),
            },
            target: AddressIdentity {
                session_name: "bob@bundle".to_string(),
                display_name: Some("Bob Dylan".to_string()),
            },
            cc: vec![AddressIdentity {
                session_name: "carol@bundle".to_string(),
                display_name: Some("Carol King".to_string()),
            }],
            authenticated_identity: Some("principal-alice".to_string()),
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        is_receipt: false,
    };

    let mailbox = Arc::new(StubMailbox::with_entries(vec![mail_entry(envelope)]));
    let mut transport = started_transport(services, &mailbox);
    wait_for_ack(&mailbox);
    transport.fence_generation();

    let incoming = captured.lock().unwrap().clone().expect("incoming captured");

    // Sender: bare canonical, never the decorating pane-header form.
    assert_eq!(incoming.sender_session, "alice@bundle");
    assert_ne!(
        incoming.sender_session, "Alice Cooper <session:alice@bundle>",
        "sender_session must stay bare canonical, never the render_address form",
    );

    // Cc: bare canonical, never the decorating pane-header form.
    assert_eq!(incoming.cc_sessions, vec!["carol@bundle".to_string()]);
    assert!(
        !incoming
            .cc_sessions
            .iter()
            .any(|cc| cc.contains("<session:")),
        "cc_sessions must stay bare canonical, never the render_address form",
    );
}
