//! Unit coverage for the first-class UI transport.
//!
//! Exercises `UiTransport`'s `mailw` broadcast (via the injected services
//! closures), the bounded reconnect timeout, and the unsupported raw-write /
//! non-lookable capability surface — all through the public `Transport` trait,
//! so the test never reaches into the relay or transport internals.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use agentmux::envelope::AddressIdentity;
use agentmux::transports::{
    DeliveryEnvelope, DeliveryMessage, GenerationFence, SendOutcome, Transport, UiBroadcastStatus,
    UiIncomingMessage, UiTransport, UiTransportServices,
};

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

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime")
        .block_on(future)
}

#[test]
fn ui_mailw_broadcasts_incoming_and_resolves_delivered() {
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

    let mut transport = UiTransport::new(services);
    let outcome = block_on(transport.mailw(ui_envelope())).expect("mailw outcome future resolves");

    assert_eq!(outcome.outcome, SendOutcome::Delivered);
    assert_eq!(outcome.message_id, "m-1");

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

/// A delivery with no UI listening resolves `not_submitted` at once, from one
/// attempt rather than a wait.
///
/// This replaces a bounded reconnect poll. The wait was an absence timer with a
/// budget only this transport knew, and elapsed time decided the outcome. What
/// it bought — a message still queued when a UI happens to come back — is worth
/// little while nothing replays to a reconnecting UI, and it cost every send to
/// an unwatched target thirty seconds before the sender heard anything.
///
/// `not_submitted` rather than a failure spelling: no subscriber received the
/// broadcast, which the transport observed rather than inferred.
#[test]
fn a_broadcast_with_no_endpoint_resolves_not_submitted() {
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

    let mut transport = UiTransport::new(services);
    let outcome = block_on(transport.mailw(ui_envelope())).expect("mailw outcome future resolves");

    assert_eq!(outcome.outcome, SendOutcome::NotSubmitted);
    assert_eq!(outcome.reason_code.as_deref(), Some("ui_no_endpoint"));
    // One attempt, not a poll. A `routed` phase that reported no endpoint used
    // to send the executor back around a sleep loop; the absence of a second
    // one is what shows the wait is gone rather than merely shortened.
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
/// The steps are driven directly rather than through `acknowledge_fence`, because
/// the protocol's windows and verdicts are covered against a controllable
/// generation in `unit/delivery_fence.rs`; what is wanted here is the real
/// transport's response to each step, without racing a timer to deliver them.
///
/// Not asserted: the absence of a child process, because no UI code path spawns
/// one and there is nothing to observe the absence of. The testable content of
/// "no owned child" is that the destructive step still bites, which is what the
/// broadcast count and the outcome pin.
#[test]
fn a_terminated_ui_generation_stops_emitting_without_a_child_to_signal() {
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
                // Park the executor inside the probe, where neither fence flag has
                // been read yet on this iteration.
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

    let mut transport = UiTransport::new(services);
    let outcome_future = transport.mailw(ui_envelope());
    wait_for(
        &probe_entered,
        "the delivery executor to enter the routed probe",
    );

    // An executor of this generation is running, so it has not ceased. Reading
    // cessation as true here is the dangerous answer: it would let a replacement
    // generation start alongside a live writer.
    assert!(
        !transport.generation_ceased(),
        "a generation with a running executor must not report cessation",
    );

    transport.fence_generation();
    transport.terminate_generation();
    released.store(true, Ordering::SeqCst);

    let outcome = block_on(outcome_future).expect("mailw outcome future resolves");

    assert_eq!(outcome.outcome, SendOutcome::NotSubmitted);
    assert_eq!(outcome.reason_code.as_deref(), Some("ui_generation_fenced"));
    // Pins the revoked rung specifically. Both rungs spell the same reason code,
    // and only the reason text says which check stopped the executor — so without
    // this, a delivery stopped by the cooperative flag would pass while proving
    // nothing about the destructive step.
    assert!(
        outcome
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("terminated")),
        "the outcome must name the revocation rather than the cooperative flag: {:?}",
        outcome.reason,
    );
    assert_eq!(
        broadcasts.load(Ordering::SeqCst),
        0,
        "a terminated generation must not emit to a subscriber",
    );

    // The observation the fence's second window would make. Polled rather than
    // read once: the executor resolves its outcome before it returns, so the
    // handle can still be live for an instant after the future settles.
    let started = Instant::now();
    while !transport.generation_ceased() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a revoked generation whose executor has resolved must cease",
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn ui_raww_is_unsupported() {
    let services = UiTransportServices {
        broadcast_incoming: Arc::new(|_| UiBroadcastStatus::Delivered),
        emit_phase: Arc::new(|_| UiBroadcastStatus::Delivered),
    };
    let mut transport = UiTransport::new(services);
    let outcome = block_on(transport.raww("raw text".to_string(), true))
        .expect("raww outcome future resolves");
    assert_eq!(outcome.outcome, SendOutcome::Failed);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("ui_raw_write_unsupported"),
    );
}

#[test]
fn ui_transport_is_ready_for_handover_and_not_lookable() {
    let services = UiTransportServices {
        broadcast_incoming: Arc::new(|_| UiBroadcastStatus::Delivered),
        emit_phase: Arc::new(|_| UiBroadcastStatus::Delivered),
    };
    let transport = UiTransport::new(services);
    assert!(transport.is_ready_for_handover());
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

    let mut transport = UiTransport::new(services);
    block_on(transport.mailw(envelope)).expect("mailw outcome future resolves");

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
