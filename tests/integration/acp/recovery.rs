use agentmux::relay::SendOutcome;
use std::{
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_next_send_recovers_after_connection_closed_failure() {
    let temporary = TempDir::new().expect("temporary");
    let failing = AcpStubOptions {
        disconnect_on_prompt: Some("before_activity".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &failing);
    let first = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let first_result = send_result(first);
    assert_eq!(first_result.outcome, SendOutcome::Queued);

    // Swap in a healthy stub before the respawn loop picks up its next
    // backoff slot so the rebuilt ACP child sees the recovered behavior.
    let recovered = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &recovered);
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(3),
        ),
        "worker did not auto-respawn back to available after disconnect"
    );
    let second = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let second_result = send_result(second);
    assert_eq!(second_result.outcome, SendOutcome::Queued);
}

#[test]
fn acp_next_send_recovers_after_post_accept_disconnect() {
    let temporary = TempDir::new().expect("temporary");
    let failing = AcpStubOptions {
        disconnect_on_prompt: Some("after_activity".to_string()),
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &failing);
    let first = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let first_result = send_result(first);
    assert_eq!(first_result.outcome, SendOutcome::Queued);

    let recovered = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &recovered);
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(3),
        ),
        "worker did not auto-respawn back to available after disconnect"
    );
    let second = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let second_result = send_result(second);
    assert_eq!(second_result.outcome, SendOutcome::Queued);
}

fn wait_for_worker_state(
    root: &std::path::Path,
    target_session: &str,
    expected: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if read_worker_state(root, target_session).as_deref() == Some(expected) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

// todos/acp/16 + todos/acp/18: the ACP reader thread must not block on the
// operator's permission decision. With the resolver hoisted onto a dedicated
// short-lived thread, an agent that emits session/request_permission and
// then exits reaches EOF on the reader's next read_line, fires on_completion
// with ConnectionClosed, transitions the worker to Unavailable, and drives
// the respawn loop. The respawn calls invalidate_pending_for_respawn, which
// wakes the parked resolver with a cancelled outcome.
//
// All three transitions are observed via in-process readiness channels
// (subscribed BEFORE dispatch, so no event can be missed). Pre-channel
// versions of this test polled permission_queue.json and the worker-state
// snapshot with 10 s budgets to absorb macOS scheduling latency. The
// channel-backed waits are deterministic; the budget reverts to 5 s.
#[test]
fn acp_respawn_recovers_when_agent_exits_during_pending_permission() {
    let temporary = TempDir::new().expect("temporary");
    let exiting = AcpStubOptions {
        request_permission_on_prompt: true,
        disconnect_on_prompt: Some("after_activity".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &exiting);

    // Subscribe BEFORE dispatch so no transitions can fire and be missed.
    let mut worker_state_receiver = subscribe_bravo_worker_state(temporary.path());
    let mut queue_receiver = subscribe_bravo_permission_queue(temporary.path());

    let first = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let first_result = send_result(first);
    assert_eq!(first_result.outcome, SendOutcome::Queued);

    // Permission record must be enqueued. Only the resolver thread (the
    // todos/acp/16 code path) calls enqueue_choice_request. If the
    // Enqueued event never arrives, the reader either parked in the handler
    // (regression to the pre-fix deadlock) or failed to spawn the resolver.
    assert!(
        await_permission_event(
            &mut queue_receiver,
            |event| matches!(
                event,
                ChoicesQueueEvent::Enqueued { target_session, .. }
                    if target_session == "bravo"
            ),
            Duration::from_secs(5),
        ),
        "resolver thread did not publish Enqueued for bravo"
    );

    // Respawn must invalidate the pending record. This is only reachable if
    // the worker observed Unavailable (i.e. on_completion(ConnectionClosed)
    // fired). The Invalidated event with the respawn reason code is the
    // critical signal that the deadlock-breaking path executed.
    assert!(
        await_permission_event(
            &mut queue_receiver,
            |event| matches!(
                event,
                ChoicesQueueEvent::Invalidated { target_session, reason_code, .. }
                    if target_session == "bravo"
                        && reason_code == "runtime_choices_request_invalidated_by_respawn"
            ),
            Duration::from_secs(5),
        ),
        "respawn did not publish Invalidated for the pending permission of bravo"
    );

    // Final state must converge to available; the respawn loop must rebuild
    // the ACP child and resume readiness.
    assert!(
        await_acp_worker_state(
            &mut worker_state_receiver,
            AcpWorkerReadinessState::Available,
            Duration::from_secs(5),
        ),
        "worker did not auto-respawn back to available after permission-then-exit"
    );
}

#[test]
fn acp_respawn_with_missing_load_capability_is_permanent_failure() {
    let temporary = TempDir::new().expect("temporary");
    // load_capability is irrelevant on the initial bootstrap because no
    // session id is persisted yet; the worker uses session/new. After the
    // disconnect, the respawn finds the persisted id and must use
    // session/load, but the agent does not advertise the capability, so the
    // respawn surfaces MISSING_CAPABILITY as a permanent failure.
    let options = AcpStubOptions {
        load_capability: false,
        disconnect_on_prompt: Some("after_activity".to_string()),
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let first = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let first_result = send_result(first);
    assert_eq!(first_result.outcome, SendOutcome::Queued);

    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "unavailable",
            Duration::from_secs(3),
        ),
        "respawn did not surface permanent failure when session/load is unsupported"
    );

    assert_acp_delivery_unavailable(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
}
