use agentmux::relay::ChatOutcome;
use serde_json::Value;
use std::{
    fs, thread,
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
    let first_result = chat_result(first);
    assert_eq!(first_result.outcome, ChatOutcome::Queued);

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
    let second_result = chat_result(second);
    assert_eq!(second_result.outcome, ChatOutcome::Queued);
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
    let first_result = chat_result(first);
    assert_eq!(first_result.outcome, ChatOutcome::Queued);

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
    let second_result = chat_result(second);
    assert_eq!(second_result.outcome, ChatOutcome::Queued);
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

fn read_pending_permission_target_sessions(runtime_directory: &std::path::Path) -> Vec<String> {
    let queue_path = runtime_directory.join("permission_queue.json");
    let Ok(raw) = fs::read_to_string(&queue_path) else {
        return Vec::new();
    };
    let Ok(parsed): Result<Value, _> = serde_json::from_str(&raw) else {
        return Vec::new();
    };
    parsed["pending"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .filter_map(|record| {
                    record
                        .get("target_session")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn wait_for_pending_permission(
    runtime_directory: &std::path::Path,
    target_session: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let pending = read_pending_permission_target_sessions(runtime_directory);
        if pending.iter().any(|candidate| candidate == target_session) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn wait_for_permission_invalidation(
    runtime_directory: &std::path::Path,
    target_session: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let pending = read_pending_permission_target_sessions(runtime_directory);
        if !pending.iter().any(|candidate| candidate == target_session) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

// Exercises `invalidate_pending_for_respawn` through the respawn loop by
// seeding a queue record directly. The natural in-flight path (agent emits
// session/request_permission then exits) is covered by
// `acp_respawn_recovers_when_agent_exits_during_pending_permission`.
#[test]
fn acp_respawn_invalidates_pending_permission_queue_entry_for_target() {
    let temporary = TempDir::new().expect("temporary");
    let failing = AcpStubOptions {
        disconnect_on_prompt: Some("after_activity".to_string()),
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &failing);

    seed_permission_queue_record(temporary.path(), "bravo", "perm-respawn-seeded");
    assert!(
        wait_for_pending_permission(temporary.path(), "bravo", Duration::from_secs(1)),
        "seeded permission record was not visible in the queue file"
    );

    let first = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let first_result = chat_result(first);
    assert_eq!(first_result.outcome, ChatOutcome::Queued);

    assert!(
        wait_for_permission_invalidation(temporary.path(), "bravo", Duration::from_secs(3)),
        "respawn did not invalidate the seeded permission record for bravo"
    );

    let recovered = AcpStubOptions::default();
    let (_config_root, _log_path) = write_configuration(temporary.path(), &recovered);
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(3)
        ),
        "worker did not return to available after respawn"
    );
}

fn seed_permission_queue_record(
    runtime_directory: &std::path::Path,
    target_session: &str,
    permission_request_id: &str,
) {
    let queue_path = runtime_directory.join("permission_queue.json");
    let state = serde_json::json!({
        "schema_version": 1,
        "next_sequence": 1,
        "pending": [
            {
                "permission_request_id": permission_request_id,
                "message_id": "msg-respawn-seeded",
                "target_session": target_session,
                "requested_kind": "exec",
                "requested_details": {
                    "options": [
                        { "option_id": "allow", "name": "Allow", "kind": "allow" }
                    ]
                },
                "enqueued_at": "2026-05-15T00:00:00Z",
                "enqueued_at_ms": 0,
                "sequence": 0,
            }
        ],
    });
    fs::write(
        &queue_path,
        serde_json::to_string(&state).expect("serialize seed"),
    )
    .expect("write seed permission queue");
}

// todos/acp/16: the ACP reader thread must not block on the operator's
// permission decision. With the resolver hoisted onto a dedicated short-lived
// thread, an agent that emits session/request_permission and then exits
// reaches EOF on the reader's next read_line, fires on_completion with
// ConnectionClosed, transitions the worker to Unavailable, and drives the
// respawn loop. The respawn calls invalidate_pending_for_respawn, which
// wakes the parked resolver with a cancelled outcome. Pre-fix, the reader
// was parked in the permission handler and never observed EOF, so this
// scenario deadlocked indefinitely.
#[test]
fn acp_respawn_recovers_when_agent_exits_during_pending_permission() {
    let temporary = TempDir::new().expect("temporary");
    let exiting = AcpStubOptions {
        request_permission_on_prompt: true,
        disconnect_on_prompt: Some("after_activity".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &exiting);
    let first = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let first_result = chat_result(first);
    assert_eq!(first_result.outcome, ChatOutcome::Queued);

    // Permission record must land in the persisted queue. Only the resolver
    // thread (the new code path) enqueues. If it never appears, the reader
    // either parked in the handler (regression to the pre-fix deadlock) or
    // failed to spawn the resolver at all.
    // 10 s budget: macOS CI runners are 3-5x slower than Linux for
    // process-heavy tests; the right fix is a readiness channel rather than
    // filesystem polling (see todos/acp/18).
    assert!(
        wait_for_pending_permission(temporary.path(), "bravo", Duration::from_secs(10)),
        "resolver thread did not enqueue a pending permission record for bravo"
    );

    // Respawn must invalidate the pending record. This is only reachable if
    // the worker observed Unavailable (i.e. on_completion(ConnectionClosed)
    // fired). With the reader unparked, that completion path is now
    // reachable during a pending permission.
    assert!(
        wait_for_permission_invalidation(temporary.path(), "bravo", Duration::from_secs(10)),
        "respawn did not invalidate the pending permission record for bravo"
    );

    // Final state must converge to available; the respawn loop must rebuild
    // the ACP child and resume readiness.
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(10),
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
    let first_result = chat_result(first);
    assert_eq!(first_result.outcome, ChatOutcome::Queued);

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
