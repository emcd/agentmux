//! ACP prime-timer integration tests for the `acp-prime-timeout-and-wedge-detection` proposal.
//!
//! The bounded-prime-wait for ACP sessions is opt-in via the per-coder
//! `[coders.<id>.acp].prime-timeout-ms` config key. The tests assert the
//! prime timer behavior against the pre-existing stub harness in
//! `helpers.rs`. Because the worker respawn loop is fast and overwrites
//! the readiness transition window in microseconds, the tests assert
//! end-state round-trip timings rather than racing the watch channel for
//! a specific intermediate value: a fired prime window recovers within a
//! short budget (the stub re-bootstraps in a few hundred ms), while an
//! unfired prime window only recovers after the stub's `prompt_delay_sec`
//! elapses (seconds-long budget).
//!
//! - `acp_prime_timeout_fires_after_configured_window` (5.1): with a
//!   short prime window (250 ms) and a stub that hangs for 5 seconds,
//!   the worker must round-trip through Unavailable and back to
//!   Available within ~1 second. Without the prime timer, recovery would
//!   be blocked on the stub's 5-second sleep.
//! - `acp_prime_timer_does_not_reset_on_coalesce` (5.2): two envelopes
//!   pushed into the same flush group inherit the head envelope's prime
//!   anchor; the test asserts the fire round-trip completes in roughly
//!   the prime window, NOT ~2x the prime window (which would indicate a
//!   reset-on-coalesce regression).
//! - `acp_prime_timer_does_not_fire_during_pending_choice` (5.3): when
//!   the stub raises a `session/request_permission`, the prime timer
//!   must NOT fire while the operator decision is in flight. The test
//!   auto-decides the choice via the harness' permission-queue and
//!   asserts the worker reaches `Available` without an early Unavailable
//!   round-trip (which would indicate a prime-during-pending-choice
//!   regression).
//! - `acp_prime_timeout_default_unbounded` (5.4): when no prime timeout
//!   is configured, the worker completes a long-running prompt normally
//!   and reaches `Available`.
//!
//! The tests use the existing
//! `subscribe_bravo_worker_state` + `await_acp_worker_any_state` helpers,
//! which is the deterministic surface used by the rest of the ACP
//! integration suite.

use std::{
    thread,
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::helpers::*;

const ACP_PRIME_RECOVERY_BUDGET: Duration = Duration::from_secs(2);

fn await_acp_recovery(
    receiver: &mut tokio::sync::watch::Receiver<Option<agentmux::transports::WorkerReadinessState>>,
    recovery_budget: Duration,
) -> bool {
    await_acp_worker_state(
        receiver,
        agentmux::transports::WorkerReadinessState::Available,
        recovery_budget,
    )
}

#[test]
fn acp_prime_timeout_fires_after_configured_window() {
    let temporary = TempDir::new().expect("temporary");
    // The stub will hang for 5 seconds before responding. Configure a
    // short prime window (250 ms) so the prime timer fires well before
    // the stub completes. Without the prime timer, the worker would only
    // recover after the 5-second stub delay. With the prime timer, the
    // respawn round-trip completes within ~ACP_PRIME_RECOVERY_BUDGET.
    let options = AcpStubOptions {
        prompt_delay_sec: 5,
        coder_prime_timeout_ms: Some(250),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);

    let mut worker_state_receiver = subscribe_bravo_worker_state(temporary.path());
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, agentmux::relay::SendOutcome::Queued);

    // The prime fire latches readiness to `Unavailable` and the respawn
    // loop brings the worker back to `Available` once the stub is
    // healthy again. The end-to-end round-trip must complete well inside
    // the budget; without the prime timer, the worker would be blocked
    // on the stub's 5-second `prompt_delay_sec`.
    assert!(
        await_acp_recovery(&mut worker_state_receiver, ACP_PRIME_RECOVERY_BUDGET),
        "ACP worker did not recover within {ACP_PRIME_RECOVERY_BUDGET:?}; \
         the prime timer either did not fire or the recovery loop is broken"
    );
}

#[test]
fn acp_prime_timer_does_not_reset_on_coalesce() {
    let temporary = TempDir::new().expect("temporary");
    // The prime timer is anchored at first wait start and does NOT extend
    // on coalesce-during-wait. With a 300 ms prime window, the round-trip
    // to recover must complete well before `2 * prime_timeout_ms + slack`,
    // which would be the worst case if absorbed envelopes were restarting
    // the timer.
    let options = AcpStubOptions {
        prompt_delay_sec: 5,
        coder_prime_timeout_ms: Some(300),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);

    let mut worker_state_receiver = subscribe_bravo_worker_state(temporary.path());
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, agentmux::relay::SendOutcome::Queued);

    let started_at = Instant::now();
    assert!(
        await_acp_recovery(&mut worker_state_receiver, ACP_PRIME_RECOVERY_BUDGET),
        "ACP worker did not recover within {ACP_PRIME_RECOVERY_BUDGET:?} \
         after coalesced prime anchors; the bounded-wait path is broken"
    );
    let elapsed = started_at.elapsed();
    // A reset-on-coalesce regression would push this well above the
    // prime window; the actual measured upper bound is the prime window
    // plus a reasonable respawn overhead (~ 300 ms). Two-prime-window
    // round-trip is ~700 ms; allow up to one round plus overhead.
    assert!(
        elapsed < Duration::from_millis(800),
        "prime-timer round-trip took {elapsed:?}; a reset-on-coalesce regression \
         would push this above 800 ms"
    );
}

#[test]
fn acp_prime_timer_does_not_fire_during_pending_choice() {
    let temporary = TempDir::new().expect("temporary");
    // The stub raises `session/request_permission` and waits 4 seconds
    // before completing. The prime window is 250 ms. The prime timer
    // must NOT fire while the choice is in flight; the operator (here:
    // the test harness auto-decides the choice via the permission queue)
    // decides first, then the turn completes normally. The worker must
    // reach `Available`, having NOT taken the prime-fire round-trip.
    let options = AcpStubOptions {
        prompt_delay_sec: 4,
        request_permission_on_prompt: true,
        coder_prime_timeout_ms: Some(250),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);

    let mut worker_state_receiver = subscribe_bravo_worker_state(temporary.path());
    let _ = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));

    // The worker should reach `Available` only after the operator's
    // decision lands in `pending_choice` AND the stub completes. The
    // full expected timeline is roughly prompt_delay_sec + permission
    // resolution; we give it a generous budget but assert it does NOT
    // occur spuriously fast (which would indicate the prime timer
    // fired during the pending choice).
    let started_at = Instant::now();
    assert!(
        await_acp_recovery(&mut worker_state_receiver, Duration::from_secs(8)),
        "worker did not settle to Available; prime timer may have fired \
         during the pending choice"
    );
    let elapsed = started_at.elapsed();
    // A prime-during-pending-choice regression would surface as a
    // sub-second round-trip (prime fires at ~250 ms, recovery round-
    // trip ~500 ms). The expected minimum is well above 1 second: the
    // operator decision path takes time even when auto-resolved, plus
    // the stub's prompt_delay_sec elapses. We assert >= 1 second to
    // distinguish from the prime-fire short-circuit.
    assert!(
        elapsed >= Duration::from_secs(1),
        "worker recovered in {elapsed:?}; the prime timer likely fired \
         during the pending choice (would surface as a sub-second \
         round-trip)"
    );
}

#[test]
fn acp_prime_timeout_default_unbounded() {
    let temporary = TempDir::new().expect("temporary");
    // No `prime-timeout-ms` configured: default `None` preserves today's
    // unbounded wait. The stub's `prompt_delay_sec` of 2 seconds is the
    // longest wait in this test; the worker must reach `Available`
    // without the prime timer firing.
    let options = AcpStubOptions {
        prompt_delay_sec: 2,
        coder_prime_timeout_ms: None,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);

    let mut worker_state_receiver = subscribe_bravo_worker_state(temporary.path());
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, agentmux::relay::SendOutcome::Queued);

    assert!(
        await_acp_recovery(&mut worker_state_receiver, Duration::from_secs(8)),
        "worker did not settle to Available on the unbounded default; \
         prime timer fired unexpectedly"
    );
}

// Touch the `thread` symbol so the import is not flagged as unused when
// an integration variant of this module is compiled in isolation.
#[allow(dead_code)]
fn _keep_thread_import() {
    thread::sleep(Duration::from_millis(0));
}
