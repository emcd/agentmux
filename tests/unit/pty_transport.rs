//! Unit coverage for Pty prompt readiness and look snapshots.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use agentmux::pty::{
    PtyConfigSnapshot, PtyOutputView, PtyPromptProbe, PtyShared, SnapshotResponse,
};
use agentmux::transports::{
    LookMode, LookSnapshotPayload, PackingUnitId, PartitionError, PartitionSink, SubmissionEvidence,
};
use regex::Regex;
use tokio::sync::mpsc;

/// A sink that accepts every declaration and remembers the evidence recorded
/// against each unit.
///
/// Accepting is what the relay does for a member it has admitted, so these tests
/// stay on the path production takes; a refusing sink would skip every write and
/// they would be testing the refusal instead of the partition.
#[derive(Default)]
struct RecordingSink {
    declared: Mutex<Vec<Vec<String>>>,
    recorded: Mutex<Vec<(PackingUnitId, SubmissionEvidence)>>,
}

impl PartitionSink for RecordingSink {
    fn declare(&self, member_ids: &[&str]) -> Result<PackingUnitId, PartitionError> {
        self.declared
            .lock()
            .expect("declared mutex")
            .push(member_ids.iter().map(|id| (*id).to_string()).collect());
        Ok(PackingUnitId::mint())
    }

    fn record(&self, unit: PackingUnitId, evidence: SubmissionEvidence) {
        self.recorded
            .lock()
            .expect("recorded mutex")
            .push((unit, evidence));
    }
}

fn shared_with(script: Vec<SnapshotResponse>) -> (PtyShared, thread::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<agentmux::pty::SnapshotRequest>(8);
    let script = Arc::new(Mutex::new(VecDeque::from(script)));
    let worker_script = Arc::clone(&script);
    let handle = thread::spawn(move || {
        while let Some(request) = rx.blocking_recv() {
            let response = worker_script
                .lock()
                .expect("script mutex")
                .pop_front()
                .unwrap_or(SnapshotResponse {
                    tail: String::new(),
                    cursor_x: 0,
                    cursor_y: 0,
                    cursor_visible: false,
                });
            let _ = request.tx.send(response);
        }
    });
    let shared = PtyShared {
        config: PtyConfigSnapshot {
            target_member_id: "test-session".to_string(),
            cols: 120,
            rows: 40,
            prompt_regex: Some(Regex::new(r"READY").expect("regex")),
            prompt_inspect_lines: 3,
            prompt_idle_column: Some(2),
        },
        snapshot_tx: tx,
        child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    (shared, handle)
}

#[test]
fn pty_prompt_probe_reports_ready_from_snapshot_and_cursor() {
    let (shared, handle) = shared_with(vec![SnapshotResponse {
        tail: "READY".to_string(),
        cursor_x: 2,
        cursor_y: 0,
        cursor_visible: true,
    }]);
    let mut probe = PtyPromptProbe::new(shared);

    assert!(probe.observe_blocking().expect("snapshot observation"));
    drop(probe);
    handle.join().expect("snapshot worker");
}

#[test]
fn pty_prompt_probe_rejects_cursor_mismatch() {
    let (shared, handle) = shared_with(vec![SnapshotResponse {
        tail: "READY".to_string(),
        cursor_x: 3,
        cursor_y: 0,
        cursor_visible: true,
    }]);
    let mut probe = PtyPromptProbe::new(shared);

    assert!(!probe.observe_blocking().expect("snapshot observation"));
    drop(probe);
    handle.join().expect("snapshot worker");
}

#[tokio::test]
async fn pty_look_returns_requested_tail() {
    let (shared, handle) = shared_with(vec![SnapshotResponse {
        tail: "one\ntwo\nthree".to_string(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: false,
    }]);
    let view = PtyOutputView::new(shared);

    let result = view
        .look_async(LookMode {
            lines: Some(2),
            offset: None,
            prime_timeout: Duration::ZERO,
        })
        .await
        .expect("look snapshot");
    assert!(matches!(
        result,
        LookSnapshotPayload::Lines { snapshot_lines }
            if snapshot_lines == vec!["two".to_string(), "three".to_string()]
    ));
    drop(view);
    handle.join().expect("snapshot worker");
}

/// A `Write` that records bytes and fails after a chosen number of successful
/// `write_all` calls. Used to drive `Delivery::start_envelope_group`'s
/// per-member evidence: a unit whose write fails must not change what a
/// sibling's already-succeeded write proved.
struct FailingWriter {
    bytes: Vec<u8>,
    writes_allowed: usize,
    writes: usize,
}

impl std::io::Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        if self.writes > self.writes_allowed {
            return Err(std::io::Error::other("simulated pty write failure"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A `Write` that always succeeds and appends into a sink the test still holds.
///
/// `FailingWriter` records bytes too, but it is boxed as `dyn Write` before the
/// delivery takes it, so nothing can read them back afterwards. Sharing the sink
/// is what makes the bytes assertable.
struct RecordingWriter {
    sink: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for RecordingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sink
            .lock()
            .expect("recording sink")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn envelope_for(
    message_id: &str,
    body: &str,
    is_receipt: bool,
) -> agentmux::transports::DeliveryEnvelope {
    use agentmux::envelope::AddressIdentity;
    use agentmux::transports::{DeliveryEnvelope, DeliveryMessage};
    DeliveryEnvelope {
        message_id: message_id.to_string(),
        message: DeliveryMessage {
            body: body.to_string(),
            created_at: "2026-08-01T00:00:00Z".to_string(),
            namespace: "test-ns".to_string(),
            sender: AddressIdentity {
                session_name: "sender@test-ns".to_string(),
                display_name: None,
            },
            target: AddressIdentity {
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

/// A member whose unit's write failed resolves `Failed` from its own evidence,
/// while a sibling whose write succeeded resolves `Delivered` — outcomes are
/// per unit, never applied to the whole group.
#[test]
fn pty_delivery_resolves_each_member_from_its_own_evidence() {
    use agentmux::pty::delivery::{Delivery, DeliveryStep};
    use agentmux::pty::transport::DeliveryCommand;
    use agentmux::transports::SendOutcome;

    let writer: Arc<Mutex<Box<dyn std::io::Write + Send>>> =
        Arc::new(Mutex::new(Box::new(FailingWriter {
            bytes: Vec::new(),
            writes_allowed: 1,
            writes: 0,
        })));
    let (write_tx, mut write_rx) = mpsc::channel::<DeliveryCommand>(8);

    let (outcome1_tx, outcome1_rx) = tokio::sync::oneshot::channel();
    let (outcome2_tx, outcome2_rx) = tokio::sync::oneshot::channel();

    write_tx
        .blocking_send(DeliveryCommand::Envelope {
            envelope: Box::new(envelope_for("member-2", "second body", false)),
            outcome_tx: outcome2_tx,
        })
        .expect("queue second envelope");

    let mut delivery = Delivery::start_envelope_group(
        Box::new(envelope_for("member-1", "first body", false)),
        outcome1_tx,
        &mut write_rx,
        &writer,
        "test-session",
        &RecordingSink::default(),
    );

    // First member's write succeeded; the second's failed.
    assert!(matches!(
        delivery.step("test-session"),
        DeliveryStep::Done { pending_raw: None }
    ));

    let outcome1 = outcome1_rx.blocking_recv().expect("member 1 outcome");
    let outcome2 = outcome2_rx.blocking_recv().expect("member 2 outcome");
    assert_eq!(outcome1.outcome, SendOutcome::Delivered);
    assert_eq!(outcome1.message_id, "member-1");
    assert_eq!(outcome2.outcome, SendOutcome::Failed);
    assert_eq!(outcome2.message_id, "member-2");
    assert_eq!(outcome2.reason_code.as_deref(), Some("pty_write_failed"));
}

/// Every member of a partitioned group gets its own bytes written, not merely
/// its own outcome.
///
/// This is the `agentmux:issues/relay/62` regression. The defect was a member
/// absorbed into a flush group *after* that group's single write had already
/// happened: it was pushed onto the group but never written anywhere, while the
/// group resolved every member identically. Its sender was told `Delivered` for
/// bytes that never left the relay.
///
/// Asserting on outcomes cannot catch that, because the outcome is precisely
/// what was untrustworthy — the defect's signature is a resolved member with no
/// bytes behind it. So this asserts on what the writer actually received.
///
/// The structural fix is that membership is fixed at partition time and every
/// unit is written afterwards, one per member, so there is no longer a window
/// between the write and the group's membership being final.
#[test]
fn pty_delivery_writes_every_member_of_a_partitioned_group() {
    use agentmux::pty::delivery::{Delivery, DeliveryStep};
    use agentmux::pty::transport::DeliveryCommand;
    use agentmux::transports::SendOutcome;

    const FIRST_BODY: &str = "RELAY62-FIRST-ENVELOPE";
    const SECOND_BODY: &str = "RELAY62-SECOND-ENVELOPE";

    let sink = Arc::new(Mutex::new(Vec::new()));
    let writer: Arc<Mutex<Box<dyn std::io::Write + Send>>> =
        Arc::new(Mutex::new(Box::new(RecordingWriter {
            sink: Arc::clone(&sink),
        })));
    let (write_tx, mut write_rx) = mpsc::channel::<DeliveryCommand>(8);

    let (outcome1_tx, outcome1_rx) = tokio::sync::oneshot::channel();
    let (outcome2_tx, outcome2_rx) = tokio::sync::oneshot::channel();

    // Queued before partition runs, so the drain absorbs it into the same group
    // as the first envelope. That co-membership is what the defect required, and
    // it is forced here rather than raced for.
    write_tx
        .blocking_send(DeliveryCommand::Envelope {
            envelope: Box::new(envelope_for("relay62-second", SECOND_BODY, false)),
            outcome_tx: outcome2_tx,
        })
        .expect("queue second envelope");

    let mut delivery = Delivery::start_envelope_group(
        Box::new(envelope_for("relay62-first", FIRST_BODY, false)),
        outcome1_tx,
        &mut write_rx,
        &writer,
        "test-session",
        &RecordingSink::default(),
    );

    assert!(matches!(
        delivery.step("test-session"),
        DeliveryStep::Done { pending_raw: None }
    ));

    // Both resolve, and both resolve Delivered. On its own this is exactly the
    // claim the defect made falsely, which is why it is not the assertion that
    // carries this test.
    let outcome1 = outcome1_rx.blocking_recv().expect("member 1 outcome");
    let outcome2 = outcome2_rx.blocking_recv().expect("member 2 outcome");
    assert_eq!(outcome1.outcome, SendOutcome::Delivered);
    assert_eq!(outcome2.outcome, SendOutcome::Delivered);

    let written = String::from_utf8_lossy(&sink.lock().expect("recording sink")).into_owned();
    assert!(
        written.contains(FIRST_BODY),
        "the first member's body never reached the writer; written: {written:?}"
    );
    assert!(
        written.contains(SECOND_BODY),
        "the member absorbed during partition resolved {:?} but its body never \
         reached the writer (relay/62); written: {written:?}",
        outcome2.outcome,
    );
}

/// A raw barrier absorbed during partition is handed back through
/// `DeliveryStep::Done` after the group's own writes, so the worker delivers it
/// next rather than losing it to the successful group.
#[test]
fn pty_delivery_returns_a_raw_barrier_absorbed_during_partition() {
    use agentmux::pty::delivery::{Delivery, DeliveryStep};
    use agentmux::pty::transport::DeliveryCommand;

    let writer: Arc<Mutex<Box<dyn std::io::Write + Send>>> =
        Arc::new(Mutex::new(Box::new(FailingWriter {
            bytes: Vec::new(),
            writes_allowed: usize::MAX,
            writes: 0,
        })));
    let (write_tx, mut write_rx) = mpsc::channel::<DeliveryCommand>(8);

    let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
    let (raw_tx, raw_rx) = tokio::sync::oneshot::channel();

    write_tx
        .blocking_send(DeliveryCommand::Raw {
            content: "raw content".to_string(),
            append_enter: false,
            outcome_tx: raw_tx,
        })
        .expect("queue raw after envelope");

    let mut delivery = Delivery::start_envelope_group(
        Box::new(envelope_for("member-1", "first body", false)),
        outcome_tx,
        &mut write_rx,
        &writer,
        "test-session",
        &RecordingSink::default(),
    );

    // The group resolves first; the raw barrier rides out on the Done step.
    let DeliveryStep::Done { pending_raw } = delivery.step("test-session");
    let pending_raw = pending_raw.expect("raw barrier returned");

    let outcome = outcome_rx.blocking_recv().expect("member outcome");
    assert_eq!(
        outcome.outcome,
        agentmux::transports::SendOutcome::Delivered
    );

    // The worker then starts a raw-only delivery from the returned barrier.
    let raw = pending_raw;
    assert_eq!(raw.content, "raw content");
    let mut raw_delivery = Delivery::start_raw(
        raw.content,
        raw.append_enter,
        raw.outcome_tx,
        &writer,
        "test-session",
    )
    .expect("raw delivery");
    assert!(matches!(
        raw_delivery.step("test-session"),
        DeliveryStep::Done { pending_raw: None }
    ));
    let raw_outcome = raw_rx.blocking_recv().expect("raw outcome");
    assert_eq!(
        raw_outcome.outcome,
        agentmux::transports::SendOutcome::Delivered
    );
}

/// A terminal-outcome receipt is written even though nothing will declare a unit
/// for it, while a peer member whose declaration is refused is not.
///
/// The two halves are one test because the contrast is the point: a refused
/// declaration MUST suppress the write, and a receipt MUST be written anyway.
/// A receipt bypasses admission, so it holds no ledger entry, and a ledger cannot
/// tell a member it never had from one that already terminalized — which is why
/// asking about a receipt returns the same refusal as asking about a terminal
/// member, and why the receipt has to be excluded from the question rather than
/// answered by it. Without the exclusion every terminal-outcome receipt routed
/// back to a Pty sender is silently dropped, which no other test would notice:
/// receipts are relay-originated, so nothing downstream is waiting on one.
#[test]
fn pty_delivery_writes_a_receipt_no_unit_covers() {
    use agentmux::pty::delivery::{Delivery, DeliveryStep};
    use agentmux::pty::transport::DeliveryCommand;

    /// Stands in for a ledger holding no entry for either member — which is what
    /// a receipt's message id always finds, since it was never admitted.
    struct RefusingSink;
    impl PartitionSink for RefusingSink {
        fn declare(&self, _member_ids: &[&str]) -> Result<PackingUnitId, PartitionError> {
            Err(PartitionError::MemberNotBindable)
        }
        fn record(&self, _unit: PackingUnitId, _evidence: SubmissionEvidence) {}
    }

    let sink = Arc::new(Mutex::new(Vec::new()));
    let writer: Arc<Mutex<Box<dyn std::io::Write + Send>>> =
        Arc::new(Mutex::new(Box::new(RecordingWriter {
            sink: Arc::clone(&sink),
        })));
    let (write_tx, mut write_rx) = mpsc::channel::<DeliveryCommand>(8);

    let (receipt_tx, receipt_rx) = tokio::sync::oneshot::channel();
    let (peer_tx, peer_rx) = tokio::sync::oneshot::channel();

    write_tx
        .blocking_send(DeliveryCommand::Envelope {
            envelope: Box::new(envelope_for("peer-member", "peer body", false)),
            outcome_tx: peer_tx,
        })
        .expect("queue peer envelope");

    let mut delivery = Delivery::start_envelope_group(
        Box::new(envelope_for("receipt-member", "receipt body", true)),
        receipt_tx,
        &mut write_rx,
        &writer,
        "test-session",
        &RefusingSink,
    );
    assert!(matches!(
        delivery.step("test-session"),
        DeliveryStep::Done { pending_raw: None }
    ));

    // The receipt was written and resolved on its own terms.
    let receipt_outcome = receipt_rx
        .blocking_recv()
        .expect("a receipt resolves through its own outcome sender");
    assert_eq!(receipt_outcome.message_id, "receipt-member");

    // The peer member's declaration was refused, so it was never written and its
    // sender was dropped unresolved — the guard owns it now and derives
    // `not_submitted` from its being unbound.
    assert!(
        peer_rx.blocking_recv().is_err(),
        "a refused declaration must suppress the write and leave the member to the guard",
    );

    let written = String::from_utf8(sink.lock().expect("recording sink").clone())
        .expect("written bytes are utf-8");
    assert!(
        written.contains("receipt body"),
        "the receipt's bytes must reach the master: {written}",
    );
    assert!(
        !written.contains("peer body"),
        "a member with no declaration behind it must not be written: {written}",
    );
}
