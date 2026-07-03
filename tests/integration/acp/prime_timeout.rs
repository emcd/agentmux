//! ACP prime-timer integration tests for the `acp-prime-timeout-and-wedge-detection` proposal.
//!
//! The bounded-prime-wait for ACP sessions is opt-in via the per-coder
//! `[coders.<id>.acp].prime-timeout-ms` config key. The tests assert the
//! prime timer behavior against the pre-existing stub harness in
//! `helpers.rs`.
//!
//! Critical detail: `subscribe_bravo_worker_state` returns a watch receiver
//! that already has the startup `Available` published before the send
//! behavior is exercised. A naive `await Available` would trivially pass
//! against the startup publish without exercising the prime timer at all.
//! The tests below follow the established subscriber pattern used by
//! `recovery.rs`: subscribe BEFORE startup, consume the startup publishes
//! until the receiver reports `Available` for the first time, and only
//! THEN dispatch the send. Post-dispatch, the tests observe the
//! post-startup transition stream via the watch channel's
//! `changed().await` so transitions are observed synchronously without
//! polling drift.
//!
//! Coverage matrix:
//! - `acp_prime_timeout_fires_after_configured_window` (5.1):
//!   short prime window + 5-second hung prompt; the test asserts
//!   `Unavailable` is observed in the post-startup transition stream
//!   within the bounded budget. Without the prime timer, the worker
//!   stays `Busy` for ~5 seconds (no `Unavailable` observed).
//! - `acp_prime_timer_does_not_fire_during_pending_choice` (5.3):
//!   prime window of 250 ms plus a permission request mid-turn. The
//!   test asserts `Unavailable` is NEVER observed post-dispatch (prime
//!   suppressed during operator interaction) and that the worker settles
//!   to `Available` via the normal completion path.
//! - `acp_prime_timeout_default_unbounded` (5.4): no prime configured;
//!   the test asserts `Unavailable` is NEVER observed post-dispatch and
//!   the worker settles to `Available` after the stub completes.
//!
//! `acp_prime_timer_does_not_reset_on_coalesce` (5.2) is covered inline
//! by `src/acp/transport.rs::envelope_batch_prime_anchor_tests`,
//! because the ACP delivery path's outer coalesce happens before
//! `submit_envelope_turn`'s wait begins (not during it) — so
//! "coalesce-during-wait" is mechanically absent on the ACP path. The
//! `EnvelopeBatch::absorb_envelope` invariant (absorbed envelopes
//! inherit the head's `prime_timeout_ms`) is the deterministic
//! unit-level test for that decision.

use std::time::{Duration, Instant};

use tempfile::TempDir;
use tokio::sync::watch;

use super::helpers::*;

/// Awaits the next watch value strictly equal to `expected` (post-startup).
/// Returns true if observed within `budget`. Polls via `changed().await`
/// so transitions are observed synchronously. If the channel already
/// reports `expected` (a previous loop observation already saw it),
/// returns true immediately.
async fn await_post_startup_state(
    receiver: &mut watch::Receiver<Option<agentmux::transports::WorkerReadinessState>>,
    expected: agentmux::transports::WorkerReadinessState,
    budget: Duration,
) -> bool {
    if *receiver.borrow() == Some(expected) {
        return true;
    }
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let step = remaining.min(Duration::from_millis(50));
        match tokio::time::timeout(step, receiver.changed()).await {
            Ok(Ok(())) => {
                if *receiver.borrow_and_update() == Some(expected) {
                    return true;
                }
            }
            Ok(Err(_)) => return false,
            Err(_) => continue,
        }
    }
    false
}

/// Asserts that `unexpected` is NEVER observed within `budget` after the
/// start of polling. Uses the watch channel's `changed()` notification so
/// transitions are observed synchronously (no busy-loop spin).
async fn assert_post_startup_state_never_observed(
    receiver: &mut watch::Receiver<Option<agentmux::transports::WorkerReadinessState>>,
    unexpected: agentmux::transports::WorkerReadinessState,
    budget: Duration,
) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let step = remaining.min(Duration::from_millis(50));
        match tokio::time::timeout(step, receiver.changed()).await {
            Ok(Ok(())) => {
                let value = *receiver.borrow_and_update();
                if let Some(observed) = value {
                    assert_ne!(
                        observed, unexpected,
                        "expected {unexpected:?} NOT to be observed, but the post-startup \
                         transition stream reported it"
                    );
                }
            }
            Ok(Err(_)) => return,
            Err(_) => continue,
        }
    }
}

/// Consumes startup publishes (Initializing + first Available).
async fn consume_startup_until_available(
    receiver: &mut watch::Receiver<Option<agentmux::transports::WorkerReadinessState>>,
    budget: Duration,
) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let step = remaining.min(Duration::from_millis(50));
        match tokio::time::timeout(step, receiver.changed()).await {
            Ok(Ok(())) => {
                if *receiver.borrow_and_update()
                    == Some(agentmux::transports::WorkerReadinessState::Available)
                {
                    return;
                }
            }
            Ok(Err(_)) => {
                panic!("worker-state channel closed during startup");
            }
            Err(_) => continue,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn acp_prime_timeout_fires_after_configured_window() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 5,
        coder_prime_timeout_ms: Some(250),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);

    let mut receiver = subscribe_bravo_worker_state(temporary.path());
    consume_startup_until_available(&mut receiver, Duration::from_secs(2)).await;

    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, agentmux::relay::SendOutcome::Queued);

    // The post-startup stream must transition to `Unavailable` within
    // the bounded-prime budget. A non-fire path stays `Busy` for ~5
    // seconds (the stub's `prompt_delay_sec`) and is rejected by the
    // budget.
    assert!(
        await_post_startup_state(
            &mut receiver,
            agentmux::transports::WorkerReadinessState::Unavailable,
            Duration::from_millis(800),
        )
        .await,
        "ACP prime timer did not transition to Unavailable within the bounded-prime \
         budget; the prime-timer fire path is broken or never wired"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn acp_prime_timer_does_not_fire_during_pending_choice() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 4,
        request_permission_on_prompt: true,
        coder_prime_timeout_ms: Some(250),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);

    let mut receiver = subscribe_bravo_worker_state(temporary.path());
    consume_startup_until_available(&mut receiver, Duration::from_secs(2)).await;

    let _ = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));

    assert_post_startup_state_never_observed(
        &mut receiver,
        agentmux::transports::WorkerReadinessState::Unavailable,
        Duration::from_secs(6),
    )
    .await;

    assert!(
        await_post_startup_state(
            &mut receiver,
            agentmux::transports::WorkerReadinessState::Available,
            Duration::from_secs(5),
        )
        .await,
        "worker did not settle to Available after the pending choice resolved; \
         the prime-timer pending-choice suppression path is broken"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn acp_prime_timeout_default_unbounded() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 2,
        coder_prime_timeout_ms: None,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);

    let mut receiver = subscribe_bravo_worker_state(temporary.path());
    consume_startup_until_available(&mut receiver, Duration::from_secs(2)).await;

    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, agentmux::relay::SendOutcome::Queued);

    assert_post_startup_state_never_observed(
        &mut receiver,
        agentmux::transports::WorkerReadinessState::Unavailable,
        Duration::from_secs(4),
    )
    .await;

    assert!(
        await_post_startup_state(
            &mut receiver,
            agentmux::transports::WorkerReadinessState::Available,
            Duration::from_secs(5),
        )
        .await,
        "worker did not settle to Available on the unbounded default"
    );
}
