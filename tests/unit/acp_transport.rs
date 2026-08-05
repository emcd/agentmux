//! Unit coverage for the ACP transport's published look handle.
//!
//! These exercise the bounded prime-wait that the `OutputView` handle now owns
//! (Slice 2b): before `startup()` runs, the transport's readiness is
//! `Initializing` and it holds no replay buffer, yet `give_output()` still
//! publishes a handle. `look()` must wait up to `prime_timeout` for the worker
//! to leave `Initializing` before returning a stale snapshot — the behavior the
//! relay look path used to provide via `await_acp_worker_prime_for_look`, now
//! living behind the handle so it survives the startup and respawn windows.

use std::time::{Duration, Instant};

use agentmux::acp::AcpTransport;
use agentmux::envelope::PromptBatchSettings;
use agentmux::relay::{LookFreshness, LookSnapshotSource};
use agentmux::transports::{LookMode, LookSnapshotPayload, Transport};

const TEST_MAX_PROMPT_TOKENS: usize = 4096;

fn test_batch_settings() -> PromptBatchSettings {
    PromptBatchSettings {
        prompt_tokens_max: TEST_MAX_PROMPT_TOKENS,
        ..Default::default()
    }
}

#[test]
fn acp_output_view_prime_waits_then_times_out_while_initializing() {
    let transport = AcpTransport::new(test_batch_settings(), None);
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
    let transport = AcpTransport::new(test_batch_settings(), None);
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
    let transport = AcpTransport::new(test_batch_settings(), None);
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
    let transport = AcpTransport::new(test_batch_settings(), None);
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
