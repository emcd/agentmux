//! Unit coverage for the first-class UI transport.
//!
//! Exercises `UiTransport`'s `mailw` broadcast (via the injected services
//! closures), the bounded reconnect timeout, and the unsupported raw-write /
//! non-lookable capability surface — all through the public `Transport` trait,
//! so the test never reaches into the relay or transport internals.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use agentmux::envelope::AddressIdentity;
use agentmux::transports::{
    DeliveryEnvelope, DeliveryMessage, SendOutcome, Transport, UiBroadcastStatus,
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
        quiet_window: Duration::from_millis(1),
        is_receipt: false,
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

#[test]
fn ui_mailw_times_out_when_no_ui_reconnects() {
    let routed_probes = Arc::new(AtomicUsize::new(0));

    let services = UiTransportServices {
        // No UI ever connects: the broadcast is never reached.
        broadcast_incoming: Arc::new(|_incoming: &UiIncomingMessage| {
            panic!("broadcast must not run while the routed probe reports NoUi")
        }),
        emit_phase: {
            let routed_probes = routed_probes.clone();
            Arc::new(move |phase| {
                if phase.phase == "routed" {
                    routed_probes.fetch_add(1, Ordering::SeqCst);
                }
                UiBroadcastStatus::NoUi
            })
        },
    };

    // Inject a short reconnect budget so the no-UI timeout path resolves in
    // milliseconds rather than blocking the full production window.
    let mut transport =
        UiTransport::new(services).with_reconnect_timeout(Duration::from_millis(50));
    let outcome = block_on(transport.mailw(ui_envelope())).expect("mailw outcome future resolves");

    // `not_submitted`, not `timeout`. The reconnect wait elapses only on the
    // `NoUi` branch, so no broadcast was ever attempted and the transport can
    // prove nothing reached a subscriber. Reporting elapsed time as its own
    // outcome would understate what the transport knows, and would collapse to
    // `submission_unknown` at the guard.
    assert_eq!(outcome.outcome, SendOutcome::NotSubmitted);
    assert_eq!(outcome.reason_code.as_deref(), Some("ui_reconnect_elapsed"));
    assert!(
        routed_probes.load(Ordering::SeqCst) >= 1,
        "the routed reconnect probe should have run at least once",
    );
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
        quiet_window: Duration::from_millis(1),
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
