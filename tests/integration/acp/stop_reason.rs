use agentmux::relay::SendOutcome;
use agentmux::transports::WorkerReadinessState;
use std::thread;
use std::time::{Duration, Instant};

use super::helpers::*;

#[test]
fn acp_cancelled_stop_reason_is_accepted_for_async_dispatch() {
    let temporary = GuardedTempDir::new();
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
    let temporary = GuardedTempDir::new();
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
}

/// No elapsed-time bound resolves an ACP turn that is merely slow. The test
/// subscribes to the worker-state watch BEFORE bootstrap, dispatches the send
/// (which runs bootstrap → Available → Busy in-process), then drives the watch
/// through a deadline well before the 1 s stub completion, collecting every
/// distinct transition observed in that window. The worker must reach `Busy`
/// and then stay there: the stub cannot answer until ~1 s, so any transition
/// at all inside the window means something resolved the turn early.
///
/// The transition-sequence assertion is event-loop based and tolerates
/// wall-clock drift across host contention: a fixed sleep that overshoots the
/// one-second stub delay would mask an early departure from `Busy` as the
/// ordinary post-completion `Available`, so the test asserts on observed
/// transitions rather than on a sampled end-state.
#[test]
fn acp_does_not_terminalize_a_delayed_but_completing_turn() {
    let temporary = GuardedTempDir::new();
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
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

    // Drive the watch through a deadline well before the 1 s stub completion.
    // Collect every distinct transition observed within the window. Assert
    // `Busy` appears, and that nothing follows it.
    let observation_window = Duration::from_millis(250);
    let mut observed: Vec<WorkerReadinessState> = Vec::new();
    let deadline = Instant::now() + observation_window;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
        if let Some(current) = *worker_state.borrow()
            && Some(current) != last_seen
        {
            observed.push(current);
            last_seen = Some(current);
        }
    }

    let busy_at = observed
        .iter()
        .position(|state| *state == WorkerReadinessState::Busy)
        .unwrap_or_else(|| panic!("expected Busy post-dispatch transition; observed={observed:?}"));

    // Nothing at all may follow `Busy` inside the window. Naming the forbidden
    // states instead would only reject the respawn shape the deleted prime
    // timer happened to produce: a bound that resolved the turn early and
    // published `Busy -> Available` would pass a respawn-only check, because
    // `Available` is not a respawn indicator and the trailing wait below would
    // find the state it was looking for already there. The turn cannot legally
    // reach any state before the stub answers at ~1 s, so the assertion is on
    // the count of subsequent transitions rather than on their identity, and it
    // holds against terminalizations nobody has written yet.
    let after_busy = &observed[busy_at + 1..];
    assert!(
        after_busy.is_empty(),
        "no elapsed-time bound may resolve a still-completing turn: the worker \
         left Busy before the stub answered; transitions after Busy={after_busy:?}, \
         full sequence={observed:?}"
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
    let temporary = GuardedTempDir::new();
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
    let temporary = GuardedTempDir::new();
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
    let temporary = GuardedTempDir::new();
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
