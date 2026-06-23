//! Unit coverage for the transport-owned pane-envelope render boundary.
//!
//! `DeliveryMessage::render_pane_envelope` is the seam that moved out of the
//! relay worker: coder transports call it to turn relay-authored structured
//! message data into RFC 822/MIME pane text. These tests drive it through the
//! public transport + envelope APIs (render then parse) so they assert the
//! mapping without reaching into transport internals.

use agentmux::envelope::parse_envelope;
use agentmux::transports::{DeliveryMessage, DeliveryParty};

fn sample_message() -> DeliveryMessage {
    DeliveryMessage {
        body: "ship it".to_string(),
        created_at: "2026-06-18T12:00:00Z".to_string(),
        namespace: "party".to_string(),
        sender: DeliveryParty {
            session: "alice@party".to_string(),
            display_name: Some("Alice".to_string()),
        },
        target: DeliveryParty {
            session: "bob@party".to_string(),
            display_name: Some("Bob".to_string()),
        },
        cc: vec![
            DeliveryParty {
                session: "carol@other".to_string(),
                display_name: None,
            },
            DeliveryParty {
                session: "dave@party".to_string(),
                display_name: Some("Dave".to_string()),
            },
        ],
        authenticated_identity: Some("principal-alice".to_string()),
    }
}

#[test]
fn render_pane_envelope_maps_structured_fields_into_headers_and_body() {
    let rendered = sample_message().render_pane_envelope("msg-7");
    let parsed = parse_envelope(&rendered).expect("rendered envelope parses");

    assert_eq!(parsed.message_id, "msg-7");
    assert_eq!(parsed.date, "2026-06-18T12:00:00Z");
    assert_eq!(parsed.from.session_name, "alice@party");
    assert_eq!(parsed.from.display_name.as_deref(), Some("Alice"));
    assert_eq!(parsed.to.len(), 1);
    assert_eq!(parsed.to[0].session_name, "bob@party");
    assert_eq!(parsed.to[0].display_name.as_deref(), Some("Bob"));
    let cc_sessions: Vec<&str> = parsed
        .cc
        .iter()
        .map(|address| address.session_name.as_str())
        .collect();
    assert_eq!(cc_sessions, vec!["carol@other", "dave@party"]);
    assert_eq!(parsed.text_body, "ship it");
}

#[test]
fn render_pane_envelope_keeps_authenticated_identity_out_of_pane_text() {
    // Attribution like the verified principal id is preserved out-of-band in the
    // relay metadata inscription, never injected into the pane envelope.
    let rendered = sample_message().render_pane_envelope("msg-7");
    assert!(
        !rendered.contains("principal-alice"),
        "authenticated identity must not appear in pane text: {rendered}",
    );
    // The namespace is likewise routing metadata, not a rendered header value.
    assert!(
        !rendered.contains("Namespace:"),
        "namespace must not be rendered as a header: {rendered}",
    );
}

#[test]
fn render_pane_envelope_without_co_recipients_omits_cc_header() {
    let mut message = sample_message();
    message.cc.clear();
    let rendered = message.render_pane_envelope("msg-8");
    let parsed = parse_envelope(&rendered).expect("rendered envelope parses");
    assert!(parsed.cc.is_empty(), "no Cc parties means no Cc recipients");
    assert!(
        !rendered.contains("Cc:"),
        "empty co-recipient set must omit the Cc header: {rendered}",
    );
}
