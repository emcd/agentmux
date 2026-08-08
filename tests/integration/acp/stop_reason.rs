use agentmux::relay::SendOutcome;
use agentmux::transports::WorkerReadinessState;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_cancelled_stop_reason_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        stop_reason: "cancelled".to_string(),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
}

#[test]
fn acp_turn_timeout_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
}

#[test]
fn acp_coder_prime_timeout_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        coder_prime_timeout_ms: Some(120),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
}

/// Regression guard for the ACP prime-timer deletion: a configured
/// `coder_prime_timeout_ms` MUST NOT produce a timed terminal outcome
/// for a delayed-but-completing stub. The test subscribes to the
/// worker-state watch BEFORE bootstrap, dispatches the send (which
/// runs bootstrap → Available → Busy in-process), then drives the
/// watch through a deadline past the would-be 120 ms prime window
/// but well before the 1 s stub completion, collecting every
/// distinct transition observed in that window. With the prime timer
/// present, the worker would publish `Unavailable` around 120 ms in
/// and re-bootstrap through `Recovering`; without the timer, the
/// worker stays `Busy` until the stub completes around 1 s. The
/// transition-sequence assertion is event-loop based and tolerates
/// wall-clock drift across host contention: a fixed sleep that
/// overshoots the one-second stub delay would mask a former
/// `Unavailable`-then-respawn transition as the pre-completion
/// `Busy`, so the test asserts on observed transitions rather than
/// on a sampled end-state.
#[test]
fn acp_configured_prime_timeout_does_not_terminalize_a_delayed_but_completing_turn() {
    let temporary = TempDir::new().expect("temporary");
    // 120 ms prime window, 1 s stub delay.
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        coder_prime_timeout_ms: Some(120),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);

    // Subscribe BEFORE bootstrap so the watch's `None` initial value is
    // the seed `last_seen`. Any subsequent transition is detectable.
    let mut worker_state = subscribe_bravo_worker_state(temporary.path());
    let mut last_seen = *worker_state.borrow();

    // Dispatch triggers bootstrap and the send; both publish into the
    // watch (`Initializing → Available → Busy`). `dispatch_send` returns
    // once the stub logs `session/prompt` or the worker becomes
    // unavailable, whichever comes first.
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    assert_eq!(send_result(response).outcome, SendOutcome::Queued);

    // Drive the watch through a deadline past the would-be 120 ms prime
    // window but well before the 1 s stub completion. Collect every
    // distinct transition observed within the window. Assert:
    //   - `Busy` appears (post-dispatch transition);
    //   - NO `Unavailable`, `Recovering`, or `Initializing` transition
    //     appears (a prime-timer fire would publish `Unavailable` and
    //     a respawn loop would publish `Recovering`).
    let prime_window = Duration::from_millis(250);
    let mut observed: Vec<WorkerReadinessState> = Vec::new();
    let deadline = Instant::now() + prime_window;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
        if let Some(current) = *worker_state.borrow()
            && Some(current) != last_seen
        {
            observed.push(current);
            last_seen = Some(current);
        }
    }

    assert!(
        observed.contains(&WorkerReadinessState::Busy),
        "expected Busy post-dispatch transition; observed={observed:?}"
    );
    let respawn_indicators = [
        WorkerReadinessState::Unavailable,
        WorkerReadinessState::Recovering,
        WorkerReadinessState::Initializing,
    ];
    let observed_respawn: Vec<_> = observed
        .iter()
        .copied()
        .filter(|s| respawn_indicators.contains(s))
        .collect();
    assert!(
        observed_respawn.is_empty(),
        "configured coder_prime_timeout_ms must not terminalize a still-completing turn; \
         observed respawn-indicator transitions={observed_respawn:?}"
    );

    // Wait for the stub to complete and the worker to return to
    // `Available` via the normal completion path (no respawn).
    assert!(
        await_acp_worker_state(
            &mut worker_state,
            WorkerReadinessState::Available,
            Duration::from_secs(5),
        ),
        "worker did not reach Available after the delayed-but-completed turn"
    );
}

#[test]
fn acp_successful_terminal_stop_reason_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    assert_eq!(result.reason_code, None);
    assert_eq!(result.reason, None);
}

#[test]
fn acp_first_activity_acceptance_prevents_late_turn_timeout_failure() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    assert_eq!(result.reason_code, None);
}

#[test]
fn acp_async_send_returns_on_dispatch_without_waiting_for_terminal_stop_reason() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 2,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let started_at = Instant::now();
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let elapsed = started_at.elapsed();
    let result = send_result(response);

    assert_eq!(result.outcome, SendOutcome::Queued);
    assert!(
        elapsed < Duration::from_secs(1),
        "expected async send to return after dispatch, elapsed={elapsed:?}"
    );
}
