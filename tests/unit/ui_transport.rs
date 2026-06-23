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

use agentmux::transports::{
    DeliveryEnvelope, DeliveryMessage, DeliveryParty, SendOutcome, Transport, UiBroadcastStatus,
    UiIncomingMessage, UiTransport, UiTransportServices,
};

fn ui_envelope(quiescence_timeout: Option<Duration>) -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: "m-1".to_string(),
        message: DeliveryMessage {
            body: "hello ui".to_string(),
            created_at: "2026-03-05T00:00:00Z".to_string(),
            namespace: "party".to_string(),
            sender: DeliveryParty {
                session: "alice@party".to_string(),
                display_name: Some("Alice".to_string()),
            },
            target: DeliveryParty {
                session: "bob@party".to_string(),
                display_name: Some("Bob".to_string()),
            },
            cc: vec![DeliveryParty {
                session: "carol@party".to_string(),
                display_name: None,
            }],
            authenticated_identity: Some("principal-alice".to_string()),
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        quiet_window: Duration::from_millis(1),
        quiescence_timeout,
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
    let outcome = block_on(transport.mailw(ui_envelope(Some(Duration::from_secs(5)))))
        .expect("mailw outcome future resolves");

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

    let mut transport = UiTransport::new(services);
    let outcome = block_on(transport.mailw(ui_envelope(Some(Duration::from_millis(50)))))
        .expect("mailw outcome future resolves");

    assert_eq!(outcome.outcome, SendOutcome::Timeout);
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
fn ui_transport_is_ready_and_not_lookable() {
    let services = UiTransportServices {
        broadcast_incoming: Arc::new(|_| UiBroadcastStatus::Delivered),
        emit_phase: Arc::new(|_| UiBroadcastStatus::Delivered),
    };
    let transport = UiTransport::new(services);
    assert!(transport.is_ready());
    assert!(transport.give_output().is_none());
}
