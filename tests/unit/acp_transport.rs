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
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use agentmux::acp::{AcpReachability, AcpTransport};
use agentmux::envelope::PromptBatchSettings;
use agentmux::protocol::DeliveryDoorbell;
use agentmux::relay::{LookFreshness, LookSnapshotSource};
use agentmux::transports::{
    DeliveryExecutorContext, LookMode, LookSnapshotPayload, Transport, UnreachableSince,
    WorkerReadinessState,
};

use crate::stub_mailbox::StubMailbox;

/// A mailbox nothing will consume, for tests that never start a transport.
///
/// Every use below drives readiness transitions or look capture and none calls
/// `startup`, so no executor exists to peek it. An empty mailbox rather than a
/// seeded one is deliberate: if one of these tests ever did spawn an executor,
/// there would be nothing for it to write, where a seeded mailbox would let it
/// reach a real `session/prompt` against no agent.
fn delivery_context() -> DeliveryExecutorContext {
    DeliveryExecutorContext {
        consumer: Arc::new(StubMailbox::empty()) as Arc<_>,
        doorbell: DeliveryDoorbell::default(),
        poll_interval: Duration::from_millis(5),
        unreachable_dwell: Duration::from_secs(30),
    }
}

/// A transport reporting itself reachable, which is what a driver that has not
/// abandoned its target hands in.
fn reachable() -> AcpReachability {
    AcpReachability::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(UnreachableSince::default()),
    )
}

fn test_transport() -> AcpTransport {
    AcpTransport::new(test_batch_settings(), None, delivery_context(), reachable())
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
    let transport = test_transport();
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
    let transport = test_transport();
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
    let transport = test_transport();
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
    let transport = test_transport();
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

/// The readiness the relay's worker-state registry mirrors, over the lifecycle
/// transitions a caller can reach without a live agent.
///
/// Whether a turn may be submitted *now* is no longer asked here — it is asked
/// inside this transport's own delivery executor, against the outstanding turn,
/// and nothing outside the crate can pose it. What is still public, and still
/// worth pinning, is the readiness an operator reads: a transport that has
/// released its runtime is `Recovering` and one that has shut down is
/// `Unavailable`, and the two must not be collapsed — a respawn monitor acts on
/// the first and must not act on the second.
///
/// Kept in `tests/unit` per the project rule: `AcpTransport` is `pub` and every
/// transition below is reached through its public surface, so inline
/// `#[cfg(test)]` would require widening or an escape hatch.
#[test]
fn readiness_transitions_and_delivery_task_handle_retention_via_public_api() {
    let transport = test_transport();
    assert_eq!(transport.readiness(), WorkerReadinessState::Initializing);

    let mut transport = test_transport();
    transport.release_runtime();
    assert_eq!(transport.readiness(), WorkerReadinessState::Recovering);

    let mut transport = test_transport();
    Transport::shutdown(&mut transport);
    assert_eq!(transport.readiness(), WorkerReadinessState::Unavailable);

    // No delivery task has been spawned yet, so there is no handle for a
    // generation supervisor to take — the field starts empty and a second
    // `take` after the first stays empty.
    let mut transport = test_transport();
    assert!(transport.take_delivery_task_handle().is_none());
    assert!(transport.take_delivery_task_handle().is_none());
}
