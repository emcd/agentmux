//! Unit coverage for Tmux handover observation and pane rendering.

use std::sync::{Arc, Mutex};

use agentmux::envelope::PromptBatchSettings;
use agentmux::tmux::{TmuxTransport, coalescing_runs, render_paste_text};
use agentmux::transports::{
    DeliveryEnvelope, DeliveryMessage, PackingUnitId, PartitionError, PartitionSink, SendOutcome,
    SubmissionEvidence, Transport,
};

/// A sink that accepts every declaration and remembers what it was told.
///
/// Declaring is what a real relay does for members it has admitted, so accepting
/// keeps the transport on the path it takes in production; the record is what
/// lets a test say which members the transport claimed shared a write.
#[derive(Default)]
struct RecordingSink {
    declared: Mutex<Vec<Vec<String>>>,
}

impl PartitionSink for RecordingSink {
    fn declare(&self, member_ids: &[&str]) -> Result<PackingUnitId, PartitionError> {
        self.declared
            .lock()
            .expect("recording sink mutex")
            .push(member_ids.iter().map(|id| (*id).to_string()).collect());
        Ok(PackingUnitId::mint())
    }

    fn record(&self, _unit: PackingUnitId, _evidence: SubmissionEvidence) {}
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
        quiet_window: std::time::Duration::from_millis(50),
        is_receipt,
    }
}

#[test]
fn tmux_handover_is_not_accepted_before_startup() {
    let transport = TmuxTransport::new(
        PromptBatchSettings::default(),
        None,
        Arc::new(RecordingSink::default()),
    );

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
    let sink = Arc::new(RecordingSink::default());
    let mut transport = TmuxTransport::new(
        PromptBatchSettings::default(),
        None,
        Arc::clone(&sink) as Arc<dyn PartitionSink>,
    );

    let outcome = Transport::mailw(&mut transport, envelope(false))
        .blocking_recv()
        .expect("stopped delivery thread must resolve mailw");
    assert_eq!(outcome.outcome, SendOutcome::Failed);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("transport_not_started")
    );
    // The refusal happens before any delivery thread exists, so no packing unit
    // was declared for this member. That is the difference between the guard
    // being able to prove `not_submitted` for it and having to fall back to
    // `submission_unknown`: binding, not the manner of the refusal, is what the
    // evidence order reads.
    assert!(
        sink.declared
            .lock()
            .expect("recording sink mutex")
            .is_empty(),
        "a write refused before startup must leave its member unbound",
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
