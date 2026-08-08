//! Unit coverage for Tmux handover observation and pane rendering.

use agentmux::envelope::PromptBatchSettings;
use agentmux::tmux::{TmuxTransport, render_paste_text};
use agentmux::transports::{DeliveryEnvelope, DeliveryMessage, SendOutcome, Transport};

fn envelope(is_receipt: bool) -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: format!("msg-{}", if is_receipt { "receipt" } else { "peer" }),
        message: DeliveryMessage {
            body: "test body".to_string(),
            created_at: "2026-08-07T00:00:00Z".to_string(),
            namespace: "test-ns".to_string(),
            sender: agentmux::envelope::AddressIdentity {
                session_name: "alpha@test-ns".to_string(),
                display_name: None,
            },
            target: agentmux::envelope::AddressIdentity {
                session_name: "target@test-ns".to_string(),
                display_name: None,
            },
            cc: Vec::new(),
            authenticated_identity: None,
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        quiet_window: std::time::Duration::from_millis(50),
        prime_timeout_ms: None,
        readiness_timeout_ms: None,
        is_receipt,
    }
}

#[test]
fn tmux_handover_is_not_accepted_before_startup() {
    let transport = TmuxTransport::new(PromptBatchSettings::default(), None);

    assert!(!transport.is_ready_for_handover());
    assert!(matches!(
        transport.health(),
        agentmux::transports::TransportHealth::Unreachable { .. }
    ));
}

#[test]
fn tmux_transport_render_paste_text_emits_receipt_marker_for_receipt_only() {
    const MARKER: &str = "--- agentmux terminal-outcome receipt ---\n";

    let receipt = render_paste_text(&envelope(true));
    assert!(receipt.starts_with(MARKER));
    assert!(receipt[MARKER.len()..].starts_with("--"));

    let peer = render_paste_text(&envelope(false));
    assert!(!peer.contains(MARKER));
}

#[test]
fn tmux_mailw_before_startup_resolves_immediately() {
    let mut transport = TmuxTransport::new(PromptBatchSettings::default(), None);

    let outcome = Transport::mailw(&mut transport, envelope(false))
        .blocking_recv()
        .expect("stopped delivery thread must resolve mailw");
    assert_eq!(outcome.outcome, SendOutcome::Failed);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("transport_not_started")
    );
}
