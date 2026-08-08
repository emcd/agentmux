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
use agentmux::transports::{LookMode, LookSnapshotPayload, OutputView};
use regex::Regex;
use tokio::sync::mpsc;

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

    assert!(probe.observe().expect("snapshot observation"));
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

    assert!(!probe.observe().expect("snapshot observation"));
    drop(probe);
    handle.join().expect("snapshot worker");
}

#[test]
fn pty_look_returns_requested_tail() {
    let (shared, handle) = shared_with(vec![SnapshotResponse {
        tail: "one\ntwo\nthree".to_string(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: false,
    }]);
    let view = PtyOutputView::new(shared);

    let result = view
        .look(LookMode {
            lines: Some(2),
            offset: None,
            prime_timeout: Duration::ZERO,
        })
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
        quiet_window: Duration::from_millis(50),
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
