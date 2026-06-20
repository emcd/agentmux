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
use agentmux::relay::{LookFreshness, LookSnapshotSource};
use agentmux::transports::{LookMode, LookSnapshotPayload, Transport};

const TEST_MAX_PROMPT_TOKENS: usize = 4096;

#[test]
fn acp_output_view_prime_waits_then_times_out_while_initializing() {
    let transport = AcpTransport::new(TEST_MAX_PROMPT_TOKENS, None);
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
    let transport = AcpTransport::new(TEST_MAX_PROMPT_TOKENS, None);
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
