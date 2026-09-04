//! Unit coverage for Pty prompt readiness, look snapshots, and the delivery
//! executor its worker thread runs.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use agentmux::configuration::{BundleMember, TargetConfiguration, TermProtocol};
use agentmux::envelope::AddressIdentity;
use agentmux::protocol::DeliveryDoorbell;
use agentmux::protocol::mailbox::{EntrySequence, MailboxEntry, MailboxPayload};
use agentmux::pty::{
    PtyConfigSnapshot, PtyOutputView, PtyShared, PtyTargetConfiguration, PtyTransport,
    SnapshotResponse, prompt_satisfied,
};
use agentmux::transports::{
    ChoiceMade, DeliveryEnvelope, DeliveryExecutorContext, DeliveryMessage, LookMode,
    LookSnapshotPayload, StartupContext, SubmissionEvidence, Transport,
};
use regex::Regex;
use tokio::sync::mpsc;

use crate::stub_mailbox::StubMailbox;

/// How long a live-pty assertion waits before failing.
///
/// Generous rather than tuned. What is being waited for is a child process
/// starting, a terminal parsing bytes, and a poll interval elapsing — none of
/// which this test controls, and a bound tight enough to be interesting would be
/// measuring the machine rather than the executor.
const LIVE_DEADLINE: Duration = Duration::from_secs(10);

fn config_snapshot() -> PtyConfigSnapshot {
    PtyConfigSnapshot {
        target_member_id: "test-session".to_string(),
        cols: 120,
        rows: 40,
        prompt_regex: Some(Regex::new(r"READY").expect("regex")),
        prompt_inspect_lines: 3,
        prompt_idle_column: Some(2),
    }
}

fn snapshot(tail: &str, cursor_x: u16) -> SnapshotResponse {
    SnapshotResponse {
        tail: tail.to_string(),
        cursor_x,
        cursor_y: 0,
        cursor_visible: true,
    }
}

/// A prompt-readiness template is satisfied only when both halves are, and each
/// half is permissive when unconfigured.
///
/// One test because the halves are one predicate: what matters is that neither
/// can carry a match on its own, which needs both the mismatching cases and the
/// unconfigured ones to be seen against each other.
#[test]
fn prompt_readiness_requires_every_configured_half() {
    let config = config_snapshot();
    assert!(prompt_satisfied(&config, &snapshot("READY", 2)));
    assert!(
        !prompt_satisfied(&config, &snapshot("READY", 3)),
        "a matching pattern must not carry a cursor at the wrong column",
    );
    assert!(
        !prompt_satisfied(&config, &snapshot("busy", 2)),
        "a cursor at the idle column must not carry a pattern that did not match",
    );

    // Unconfigured halves are permissive. A target the operator said nothing
    // about is one delivery must not withhold itself from, so absence has to
    // read as "no constraint" rather than as "not ready".
    let unconstrained = PtyConfigSnapshot {
        prompt_regex: None,
        prompt_idle_column: None,
        ..config_snapshot()
    };
    assert!(prompt_satisfied(&unconstrained, &snapshot("anything", 47)));
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
        config: config_snapshot(),
        snapshot_tx: tx,
        child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    (shared, handle)
}

#[tokio::test]
async fn pty_look_returns_requested_tail() {
    let (shared, handle) = shared_with(vec![snapshot("one\ntwo\nthree", 0)]);
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

fn envelope(message_id: &str, body: &str, is_receipt: bool) -> DeliveryEnvelope {
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

fn entry(sequence: u64, message_id: &str, body: &str, is_receipt: bool) -> MailboxEntry {
    MailboxEntry {
        sequence: EntrySequence::new(sequence).expect("a position is never zero"),
        message_id: message_id.to_string(),
        canonical_bytes: body.len() as u64,
        payload: MailboxPayload::Mail(Arc::new(envelope(message_id, body, is_receipt))),
    }
}

/// Starts a Pty transport against a child that appends every line it reads to
/// `report`, so what the child actually received is readable afterwards.
///
/// The child rather than the terminal is the observer, and deliberately: a
/// terminal tail says what the transport's own parser made of the bytes, while
/// the report says what came out the far side of the pty. The claim under test is
/// about bytes reaching the master, so the further-downstream reading is the one
/// that carries it. A `read`/`printf` loop rather than `cat` because it appends
/// per line, leaving no buffering question between the write and the assertion.
///
/// No prompt-readiness template, so the executor's readiness check is
/// unconstrained and the entries are written as soon as they are peeked. That is
/// the point: what is under test is what the executor does with a mailbox, not
/// when it decides a shell is at a prompt.
fn started_pty(
    mailbox: &Arc<StubMailbox>,
    runtime: &std::path::Path,
    report: &std::path::Path,
) -> PtyTransport {
    let command = format!(
        "sh -c 'while IFS= read -r line; do printf \"%s\\n\" \"$line\" >> {}; done'",
        report.display(),
    );
    started_pty_running(mailbox, runtime, command, Duration::from_secs(30))
}

/// Starts a Pty transport against an arbitrary child and dwell.
fn started_pty_running(
    mailbox: &Arc<StubMailbox>,
    runtime: &std::path::Path,
    command: String,
    unreachable_dwell: Duration,
) -> PtyTransport {
    let mut transport = PtyTransport::new(
        BundleMember {
            id: "test-session".to_string(),
            name: None,
            working_directory: None,
            target: TargetConfiguration::Ui,
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        },
        PtyTargetConfiguration {
            initial_command: command.clone(),
            resume_command: command,
            prompt_readiness: None,
            cols: 120,
            rows: 40,
            working_directory: None,
            term_protocol: TermProtocol::default(),
        },
        None,
        DeliveryExecutorContext {
            consumer: Arc::clone(mailbox) as Arc<_>,
            doorbell: DeliveryDoorbell::default(),
            poll_interval: Duration::from_millis(10),
            unreachable_dwell,
        },
    );
    transport
        .startup(StartupContext {
            namespace: "test-ns".to_string(),
            runtime_directory: runtime.to_path_buf(),
            target_member: BundleMember {
                id: "test-session".to_string(),
                name: None,
                working_directory: None,
                target: TargetConfiguration::Ui,
                coder_session_id: None,
                policy_id: None,
                environment: Vec::new(),
            },
            choose: Arc::new(|_| ChoiceMade::Cancelled {
                decided_by: String::new(),
                reason_code: "not_applicable".to_string(),
                reason: None,
            }),
        })
        .expect("pty transport startup");
    transport
}

/// Every peeked entry is written as its own packing unit, and every one of them
/// actually reaches the master.
///
/// This is the `agentmux:issues/relay/62` regression, restated for the pull
/// model. The defect was a member absorbed into a flush group *after* that
/// group's single write had already happened: it was resolved along with its
/// groupmates while its own bytes went nowhere, so its sender was told
/// `Delivered` for a write that never occurred.
///
/// Asserting on evidence alone cannot catch that, because the evidence is
/// precisely what was untrustworthy — the defect's signature is an acknowledged
/// member with no bytes behind it. So this asserts on both: what the executor
/// reported, and what the terminal actually received.
///
/// The receipt is in the run deliberately. A terminal-outcome receipt carries a
/// marker line that identifies it, and a write that carried it beside a peer's
/// message would render as one message to the agent reading it, with the marker
/// stranded in the middle of somebody else's text. Pty writes one member per
/// primitive, so the separation is structural here rather than a packing
/// decision — and a plan that ever coalesced would show up as a marker the
/// terminal received in the wrong place.
#[test]
fn pty_writes_each_peeked_entry_as_its_own_unit() {
    const PEER_BODY: &str = "RELAY62-PEER-ENVELOPE";
    const RECEIPT_BODY: &str = "RELAY62-RECEIPT-ENVELOPE";

    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let report = temporary.path().join("received");
    let mailbox = Arc::new(StubMailbox::with_entries(vec![
        entry(1, "relay62-peer", PEER_BODY, false),
        entry(2, "relay62-receipt", RECEIPT_BODY, true),
    ]));
    let mut transport = started_pty(&mailbox, temporary.path(), &report);

    let started = Instant::now();
    while mailbox.acked().len() < 2 {
        assert!(
            started.elapsed() < LIVE_DEADLINE,
            "the executor acknowledged {:?} of two entries",
            mailbox.acked(),
        );
        thread::sleep(Duration::from_millis(20));
    }

    let acked = mailbox.acked();
    assert!(
        acked
            .iter()
            .all(|member| member.evidence == SubmissionEvidence::Submitted),
        "a write into a live master proves submission: {acked:?}",
    );
    assert_eq!(
        acked
            .iter()
            .map(|member| member.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["relay62-peer", "relay62-receipt"],
        "both members must be acknowledged, in mailbox order",
    );
    // One unit per entry, which is what keeps a failing write from smearing its
    // evidence across a groupmate. Read from the declared ranges rather than from
    // the peeks: a peek may legitimately return a run of two — Pty's peek
    // dimensions permit it — and what matters is that the plan covered only the
    // head of that run, so the relay bound one entry per unit.
    assert_eq!(
        mailbox.acked_units(),
        vec![
            vec![EntrySequence::new(1).expect("a position is never zero")],
            vec![EntrySequence::new(2).expect("a position is never zero")],
        ],
        "pty must declare one entry per unit however much a peek returns",
    );

    // What came out the far side of the pty, which is the half the defect could
    // not fake. Polled rather than read once: the acknowledgment says the write
    // returned, and the child's own read of those bytes follows it.
    let started = Instant::now();
    let received = loop {
        let received = std::fs::read_to_string(&report).unwrap_or_default();
        if received.contains(PEER_BODY) && received.contains(RECEIPT_BODY) {
            break received;
        }
        assert!(
            started.elapsed() < LIVE_DEADLINE,
            "the child never received both bodies; last read: {received:?}",
        );
        thread::sleep(Duration::from_millis(20));
    };

    // The receipt's marker line arrives with the receipt and not inside the
    // peer's text, which is what one-member-per-write buys. A plan that
    // coalesced the two would leave the marker stranded mid-message.
    let marker_line = received
        .lines()
        .position(|line| line.contains("terminal-outcome receipt"))
        .expect("a receipt carries its marker line");
    let receipt_line = received
        .lines()
        .position(|line| line.contains(RECEIPT_BODY))
        .expect("the receipt body reached the child");
    let peer_line = received
        .lines()
        .position(|line| line.contains(PEER_BODY))
        .expect("the peer body reached the child");
    assert!(
        peer_line < marker_line && marker_line < receipt_line,
        "the marker must precede the receipt it announces and follow the peer it \
         is not part of: peer at {peer_line}, marker at {marker_line}, receipt at \
         {receipt_line}",
    );

    Transport::shutdown(&mut transport);
}

/// A departed child's queued mail resolves at the dwell rather than waiting
/// forever.
///
/// Two separate defects would each strand it, and this test fails on either, so
/// it is written as one case rather than two.
///
/// The first is a stop condition: treating a child's exit as a reason to end the
/// executor leaves the target with no consumer at the exact moment its entries
/// need one. Unreachability is not a stop — it is the thing the dwell exists to
/// carry, and only a running executor observes it.
///
/// The second is a moving clock: reporting `Unreachable { since: Instant::now() }`
/// on every poll restamps the instant the dwell is measured from, so
/// `since.elapsed()` never reaches the threshold however long the child has been
/// gone. Latching the first observation is what makes elapsed time mean anything.
///
/// Asserted over `resolve_unreachable` rather than only over the mailbox
/// emptying, because those are different claims: the relay owns the transition
/// and chooses the outcome, and what the executor owes is the report that the
/// condition has held long enough.
#[test]
fn a_departed_pty_child_resolves_its_queued_mail_at_the_dwell() {
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let mailbox = Arc::new(StubMailbox::with_entries(vec![entry(
        1,
        "stranded",
        "NOBODY-WILL-READ-THIS",
        false,
    )]));
    // Exits at once, so the reader sees EOF and latches `child_exited` before the
    // executor has anything to write.
    let mut transport = started_pty_running(
        &mailbox,
        temporary.path(),
        "sh -c 'exit 0'".to_string(),
        Duration::from_millis(200),
    );

    let started = Instant::now();
    while mailbox.unreachable_resolutions() == 0 {
        assert!(
            started.elapsed() < LIVE_DEADLINE,
            "a departed child's entries were never resolved: acked={:?} peeks={:?}",
            mailbox.acked(),
            mailbox.peeks(),
        );
        thread::sleep(Duration::from_millis(20));
    }

    assert!(
        mailbox.is_drained(),
        "the resolved entry must leave the mailbox: {:?}",
        mailbox.acked(),
    );
    assert!(
        mailbox.acked().is_empty(),
        "nothing was written to a departed child, so nothing may be acknowledged: {:?}",
        mailbox.acked(),
    );

    Transport::shutdown(&mut transport);
}
