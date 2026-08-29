//! Unit coverage for the ACP transport's published look handle.
//!
//! These exercise the bounded prime-wait that the `OutputView` handle now owns
//! (Slice 2b): before `startup()` runs, the transport's readiness is
//! `Initializing` and it holds no replay buffer, yet `give_output()` still
//! publishes a handle. `look()` must wait up to `prime_timeout` for the worker
//! to leave `Initializing` before returning a stale snapshot — the behavior the
//! relay look path used to provide via `await_acp_worker_prime_for_look`, now
//! living behind the handle so it survives the startup and respawn windows.

use std::sync::Arc;
use std::time::{Duration, Instant};

use agentmux::acp::AcpTransport;
use agentmux::envelope::{AddressIdentity, PromptBatchSettings};
use agentmux::relay::{LookFreshness, LookSnapshotSource};
use agentmux::transports::{
    DeliveryEnvelope, DeliveryMessage, LookMode, LookSnapshotPayload, PackingUnitId,
    PartitionError, PartitionSink, SendOutcome, SubmissionEvidence, Transport,
    WorkerReadinessState,
};

/// A sink for tests that never submit a turn.
///
/// Every use below drives readiness predicates, look capture, or handover state
/// and reaches no `session/prompt`, so nothing is ever declared. Refusing rather
/// than accepting is deliberate: if one of these tests ever did reach a
/// submission, the turn would produce no effect and the test would notice,
/// whereas an accepting stub would let it write with a unit the ledger never
/// issued.
fn no_declarations_sink() -> Arc<dyn PartitionSink> {
    struct NoDeclarations;
    impl PartitionSink for NoDeclarations {
        fn declare(&self, _member_ids: &[&str]) -> Result<PackingUnitId, PartitionError> {
            Err(PartitionError::MemberNotBindable)
        }
        fn record(&self, _unit: PackingUnitId, _evidence: SubmissionEvidence) {}
    }
    Arc::new(NoDeclarations)
}

const TEST_MAX_PROMPT_TOKENS: usize = 4096;

fn test_batch_settings() -> PromptBatchSettings {
    PromptBatchSettings {
        prompt_tokens_max: TEST_MAX_PROMPT_TOKENS,
        ..Default::default()
    }
}

#[test]
fn acp_output_view_prime_waits_then_times_out_while_initializing() {
    let transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    let view = transport
        .give_output()
        .expect("ACP transport always publishes a handle");

    let prime_timeout = Duration::from_millis(150);
    let started = Instant::now();
    let snapshot = view
        .look(LookMode {
            lines: Some(5),
            offset: Some(0),
            prime_timeout,
        })
        .expect("look should not error");
    let elapsed = started.elapsed();

    // It actually waited through the prime window (slack below the full timeout
    // for poll granularity).
    assert!(
        elapsed >= Duration::from_millis(100),
        "look should prime-wait while Initializing; waited {elapsed:?}",
    );

    let LookSnapshotPayload::StructuredEntries {
        snapshot_entries,
        entries_total,
        freshness,
        snapshot_source,
        stale_reason_code,
        ..
    } = snapshot
    else {
        panic!("expected ACP entries payload");
    };
    assert!(snapshot_entries.is_empty());
    assert_eq!(entries_total, 0);
    assert_eq!(freshness, LookFreshness::Stale);
    assert_eq!(snapshot_source, LookSnapshotSource::None);
    assert_eq!(
        stale_reason_code.as_deref(),
        Some("acp_snapshot_prime_timeout"),
    );
}

#[test]
fn acp_output_view_zero_prime_timeout_returns_immediately() {
    let transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    let view = transport.give_output().expect("handle");

    let started = Instant::now();
    let snapshot = view
        .look(LookMode {
            lines: Some(5),
            offset: Some(0),
            prime_timeout: Duration::ZERO,
        })
        .expect("look should not error");
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "a zero prime_timeout must not wait",
    );

    let LookSnapshotPayload::StructuredEntries {
        freshness,
        stale_reason_code,
        ..
    } = snapshot
    else {
        panic!("expected ACP entries payload");
    };
    assert_eq!(freshness, LookFreshness::Stale);
    assert_eq!(
        stale_reason_code.as_deref(),
        Some("acp_snapshot_prime_timeout"),
    );
}

/// The respawn signal must survive a retirement decision that was made about an
/// earlier cause.
///
/// The monitor classifies an outstanding cause as answered, releases the
/// transport lock, and only then writes that decision down. A live delivery can
/// publish a genuine new cause inside that window. A signal that is a flag
/// cannot tell the two apart, so writing the decision erases the new cause — and
/// because the readiness gate withholds exactly the writes that would raise it
/// again, that is not a delayed recovery but a permanent one.
///
/// Retirement bounds the epoch it classified rather than clearing what is
/// current, so the newer cause outlives the older cause's answer by arithmetic.
#[test]
fn retiring_a_classified_cause_leaves_a_cause_published_since_it_outstanding() {
    let transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    assert_eq!(
        transport.respawn_signal_outstanding(),
        None,
        "a fresh transport owes no respawn"
    );

    transport.signal_respawn();
    let classified = transport
        .respawn_signal_outstanding()
        .expect("the first cause is outstanding once raised");

    // The window: the monitor has decided `classified` is answered and has not
    // yet said so when a second failure publishes its own cause.
    transport.signal_respawn();

    let remaining = transport.retire_respawn_signal(classified);
    let remaining = remaining.expect("the cause published since must still be outstanding");
    assert!(
        remaining > classified,
        "retirement must bound only the classified cause, leaving the newer one to be answered"
    );
}

/// Retirement is a high-water mark, so a decision that lands out of order cannot
/// resurrect a cause that a later retirement already answered.
#[test]
fn retirement_never_moves_backwards() {
    let transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    transport.signal_respawn();
    let first = transport
        .respawn_signal_outstanding()
        .expect("first cause outstanding");
    transport.signal_respawn();
    let second = transport
        .respawn_signal_outstanding()
        .expect("second cause outstanding");

    assert_eq!(
        transport.retire_respawn_signal(second),
        None,
        "retiring the current cause answers everything raised so far"
    );
    assert_eq!(
        transport.retire_respawn_signal(first),
        None,
        "a late retirement of an older cause must not resurrect an answered one"
    );
}

fn test_envelope(message_id: &str) -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: message_id.to_string(),
        message: DeliveryMessage {
            body: format!("body {message_id}"),
            created_at: "1970-01-01T00:00:00Z".to_string(),
            namespace: "party".to_string(),
            sender: AddressIdentity {
                session_name: "alpha".to_string(),
                display_name: None,
            },
            target: AddressIdentity {
                session_name: "beta".to_string(),
                display_name: None,
            },
            cc: vec![],
            authenticated_identity: None,
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: vec![],
        is_receipt: false,
    }
}

/// A Closed write channel means the delivery task has exited, so `mailw` and
/// `raww` must refuse with `not_submitted` and publish `Unavailable` rather
/// than linger `Busy` masking a dead executor. The guard's receiver is dropped
/// at test scope end (no leak); dropping the guard closes the channel.
#[test]
fn mailw_and_raww_on_closed_channel_publish_unavailable() {
    let mut transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    let guard = transport.install_write_channel_for_testing(false);
    drop(guard);

    let outcome = Transport::mailw(&mut transport, test_envelope("m1"))
        .try_recv()
        .expect("mailw refusal resolves synchronously");
    assert_eq!(outcome.outcome, SendOutcome::NotSubmitted);
    assert_eq!(
        transport.readiness(),
        WorkerReadinessState::Unavailable,
        "closed channel must publish Unavailable, not linger Busy",
    );

    let mut transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    let guard = transport.install_write_channel_for_testing(false);
    drop(guard);

    let outcome = Transport::raww(&mut transport, "hello".to_string(), true)
        .try_recv()
        .expect("raww refusal resolves synchronously");
    assert_eq!(outcome.outcome, SendOutcome::NotSubmitted);
    assert_eq!(
        transport.readiness(),
        WorkerReadinessState::Unavailable,
        "closed channel must publish Unavailable, not linger Busy",
    );
}

/// A Full write channel means the delivery task is alive but saturated, so the
/// refusal keeps `Busy` truthful until the live task drains and settles
/// `Available`. The guard (owning the receiver) is retained for the test's
/// lifetime so the channel stays open and prefilled.
#[test]
fn mailw_and_raww_on_full_channel_stay_busy() {
    let mut transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    let _guard = transport.install_write_channel_for_testing(true);

    let outcome = Transport::mailw(&mut transport, test_envelope("m2"))
        .try_recv()
        .expect("mailw refusal resolves synchronously");
    assert_eq!(outcome.outcome, SendOutcome::NotSubmitted);
    assert_eq!(
        transport.readiness(),
        WorkerReadinessState::Busy,
        "full channel keeps Busy while the delivery task is alive and saturated",
    );

    let mut transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    let _guard = transport.install_write_channel_for_testing(true);

    let outcome = Transport::raww(&mut transport, "hello".to_string(), true)
        .try_recv()
        .expect("raww refusal resolves synchronously");
    assert_eq!(outcome.outcome, SendOutcome::NotSubmitted);
    assert_eq!(
        transport.readiness(),
        WorkerReadinessState::Busy,
        "full channel keeps Busy while the delivery task is alive and saturated",
    );
}

/// Handover readiness is the narrow predicate: only `Available` qualifies.
/// This exercises the public `Transport` surface without reaching private
/// `set_readiness` — the matrix is driven via `install_write_channel_for_testing`,
/// `mailw` (Busy), `release_runtime` (Recovering), and `shutdown` (Unavailable).
/// Kept in `tests/unit` per the project rule: `AcpTransport` is `pub` and the
/// handover predicate is exercised via its public `Transport` impl, so inline
/// `#[cfg(test)]` would require widening or an escape hatch.
#[tokio::test]
async fn handover_readiness_matrix_and_delivery_task_handle_retention_via_public_api() {
    // Initial is `Initializing` — not ready for handover.
    let mut transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    assert_eq!(transport.readiness(), WorkerReadinessState::Initializing);
    assert!(!transport.is_ready_for_handover().await);

    // `Available` is the single handover-ready state. The test seam puts the
    // transport into `Available` without a live delivery task.
    let guard = transport.install_write_channel_for_testing(false);
    assert_eq!(transport.readiness(), WorkerReadinessState::Available);
    assert!(transport.is_ready_for_handover().await);

    // `Busy` is intentionally NOT handover-ready — accepting another batch
    // while a turn is in flight would dispatch the wrong message to the same
    // turn. `mailw` marks `Busy` synchronously on successful enqueue; the
    // future stays pending until a delivery task would resolve it, but the
    // readiness transition is immediate and observable here.
    let _pending = Transport::mailw(&mut transport, test_envelope("m_busy"));
    assert_eq!(transport.readiness(), WorkerReadinessState::Busy);
    assert!(!transport.is_ready_for_handover().await);
    drop(guard);

    // `Recovering` and `Unavailable` are also not ready. `release_runtime`
    // and `shutdown` are the public transitions that reach them without
    // private `set_readiness`.
    let mut transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    transport.release_runtime();
    assert_eq!(transport.readiness(), WorkerReadinessState::Recovering);
    assert!(!transport.is_ready_for_handover().await);

    let mut transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    Transport::shutdown(&mut transport);
    assert_eq!(transport.readiness(), WorkerReadinessState::Unavailable);
    assert!(!transport.is_ready_for_handover().await);

    // No delivery task has been spawned yet, so there is no handle for a
    // generation supervisor to take — the field starts empty and a second
    // `take` after the first stays empty.
    let mut transport = AcpTransport::new(test_batch_settings(), None, no_declarations_sink());
    assert!(transport.take_delivery_task_handle().is_none());
    assert!(transport.take_delivery_task_handle().is_none());
}
