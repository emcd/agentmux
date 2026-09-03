//! Unit coverage for Tmux health observation, coalescing, and pane rendering.

use std::sync::Arc;
use std::time::Duration;

use agentmux::envelope::PromptBatchSettings;
use agentmux::protocol::DeliveryDoorbell;
use agentmux::protocol::mailbox::{EntrySequence, MailboxEntry, MailboxPayload};
use agentmux::tmux::{TmuxTransport, coalescing_runs, render_paste_text};
use agentmux::transports::{DeliveryEnvelope, DeliveryExecutorContext, DeliveryMessage, Transport};

use crate::stub_mailbox::StubMailbox;

fn delivery_context(mailbox: &Arc<StubMailbox>) -> DeliveryExecutorContext {
    DeliveryExecutorContext {
        consumer: Arc::clone(mailbox) as Arc<_>,
        doorbell: DeliveryDoorbell::default(),
        poll_interval: Duration::from_millis(5),
        unreachable_dwell: Duration::from_secs(30),
    }
}

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
        is_receipt,
    }
}

#[test]
fn tmux_is_unreachable_before_startup() {
    let mailbox = Arc::new(StubMailbox::empty());
    let transport = TmuxTransport::new(
        PromptBatchSettings::default(),
        None,
        delivery_context(&mailbox),
    );

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

/// A transport that was never started consumes nothing from its target's
/// mailbox.
///
/// Under the pull model this is what "not started" *means*: the executor is
/// spawned by `startup` and nothing else, so a transport that never started has
/// nobody to peek on its behalf. The claim has teeth because the executor could
/// plausibly have been spawned at construction — the context is available there,
/// and it would work — and a transport that peeked before its runtime existed
/// would declare units it could not write, binding members the guard would then
/// have to resolve `submission_unknown` rather than proving they were never
/// submitted.
///
/// Asserted over peeks rather than declarations. A peek is the first thing an
/// executor does, so an executor that ran and then declined to declare is still
/// caught, where a declaration-only assertion would pass.
#[test]
fn tmux_consumes_nothing_before_startup() {
    let mailbox = Arc::new(StubMailbox::with_entries(vec![MailboxEntry {
        sequence: EntrySequence::first(),
        message_id: "msg-peer".to_string(),
        canonical_bytes: 9,
        payload: MailboxPayload::Mail(Arc::new(envelope(false))),
    }]));
    let _transport = TmuxTransport::new(
        PromptBatchSettings::default(),
        None,
        delivery_context(&mailbox),
    );

    // Long enough for an executor spawned at construction to have completed
    // several poll intervals, so an empty record is evidence rather than a race
    // the assertion happened to win.
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        mailbox.peeks().is_empty(),
        "a transport with no executor must consume nothing: {:?}",
        mailbox.peeks(),
    );
    assert!(
        !mailbox.is_drained(),
        "the seeded entry must still be waiting for a started transport",
    );
}

/// A terminal-outcome receipt never shares a paste with peer traffic.
///
/// The rule exists because the two have different fates. Peer members belong to
/// a packing unit and a refused declaration obliges the transport to produce no
/// effect for them; a receipt belongs to no unit and needs no declaration. Put
/// them in one prompt and the refusal takes the receipt down with the peers — and
/// it cannot be rescued afterwards, because the prompt is one combined string
/// with no receipt-only text left to write. The sender of a message that failed
/// to deliver then silently never learns it failed.
///
/// Asserted over the run split rather than through a paste, because reaching
/// `paste_group` needs a live tmux pane while the separation that makes the
/// refusal path safe is decided here.
#[test]
fn tmux_never_coalesces_a_receipt_with_peer_traffic() {
    // A receipt between peers splits the run three ways rather than joining
    // either neighbour.
    assert_eq!(coalescing_runs(&[false, false, true, false]), vec![2, 1, 1]);
    // Consecutive receipts do not coalesce with each other either: each is its
    // own turn, matching how ACP treats them as flush barriers.
    assert_eq!(coalescing_runs(&[true, true]), vec![1, 1]);
    // Peers alone still coalesce, which is the whole point of batching.
    assert_eq!(coalescing_runs(&[false, false, false]), vec![3]);
    assert_eq!(coalescing_runs(&[]), Vec::<usize>::new());

    // Whatever the split, every member lands in exactly one run and order is
    // preserved — a run table that dropped or duplicated a member would lose or
    // double-write it.
    for flags in [
        vec![true, false, false, true],
        vec![false, true, true, false, false],
        vec![true],
    ] {
        assert_eq!(
            coalescing_runs(&flags).iter().sum::<usize>(),
            flags.len(),
            "runs must partition the group exactly: {flags:?}",
        );
    }
}
