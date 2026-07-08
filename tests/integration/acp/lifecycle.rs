use std::{
    fs, thread,
    time::{Duration, Instant},
};

use agentmux::relay::SendOutcome;
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_send_selects_session_new_without_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let state_path = persisted_state_path(temporary.path(), "bravo");
    assert!(
        state_path.is_file(),
        "missing state file: {}",
        state_path.display()
    );
    let persisted: Value = serde_json::from_str(
        fs::read_to_string(&state_path)
            .expect("read state file")
            .as_str(),
    )
    .expect("parse state json");
    assert_eq!(persisted["schema_version"], 2);
    assert_eq!(persisted["acp_session_id"], "sess-generated");

    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/new\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/load\""), "log={log}");
}

#[test]
fn acp_send_reuses_persistent_worker_across_requests() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let first = dispatch_send(&config_root, &tmux_socket);
    let second = dispatch_send(&config_root, &tmux_socket);
    let first_result = send_result(first);
    let second_result = send_result(second);
    assert_eq!(first_result.outcome, SendOutcome::Queued);
    assert_eq!(second_result.outcome, SendOutcome::Queued);

    let requests = wait_for_request_count(log_path.as_path(), "session/prompt", 2);
    // Startup-owned ACP workers initialize once per configured ACP session
    // in the bundle (alpha + bravo), and subsequent sends reuse those workers.
    assert_eq!(request_count_by_method(&requests, "initialize"), 2);
    assert_eq!(request_count_by_method(&requests, "session/new"), 2);
    assert_eq!(request_count_by_method(&requests, "session/prompt"), 2);
}

#[test]
fn acp_initialize_request_uses_protocol_version_integer_and_client_version() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let requests = read_request_log(log_path.as_path());
    let initialize = request_by_method(requests.as_slice(), "initialize");
    let params = initialize.get("params").expect("initialize params object");

    assert_eq!(params["protocolVersion"], 1);
    assert_eq!(params["clientInfo"]["name"], "agentmux-relay");
    assert!(
        params["clientInfo"]["version"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(params["clientInfo"].get("title").is_none());
    assert_eq!(params["clientCapabilities"]["terminal"], false);
    assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], false);
    assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], false);
}

#[test]
fn acp_session_setup_requests_include_mcp_servers_array() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let requests = read_request_log(log_path.as_path());
    let session_new = request_by_method(requests.as_slice(), "session/new");
    assert_eq!(
        session_new["params"]["mcpServers"],
        Value::Array(Vec::new())
    );

    let second_temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        configured_session_id: Some("sess-configured".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(second_temporary.path(), &options);
    let response = dispatch_send(&config_root, &second_temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let requests = read_request_log(log_path.as_path());
    let session_load = request_by_method(requests.as_slice(), "session/load");
    assert_eq!(
        session_load["params"]["mcpServers"],
        Value::Array(Vec::new())
    );
}

#[test]
fn acp_send_uses_persisted_session_id_when_config_id_is_absent() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        disconnect_on_prompt: Some("after_activity".to_string()),
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let first = dispatch_send(&config_root, &tmux_socket);
    let first_result = send_result(first);
    assert_eq!(first_result.outcome, SendOutcome::Queued);

    // After bravo's disconnect, auto-respawn rebuilds the worker using the
    // session id persisted by the first bootstrap. Wait until the respawn
    // actually issues session/load before asserting; the log is shared with
    // alpha's stub so we wait on a content signal rather than worker state.
    assert!(
        wait_for_log_match(
            &log_path,
            "\"method\":\"session/load\"",
            Duration::from_secs(3),
        ),
        "respawn did not issue session/load within timeout"
    );

    let log = fs::read_to_string(log_path).expect("read ACP log");
    // Two session/new from initial bootstrap (alpha + bravo, sharing the
    // stub script), zero additional new from the respawn path.
    assert_eq!(
        log.matches("\"method\":\"session/new\"").count(),
        2,
        "expected one session/new per initial bootstrap (alpha+bravo), log={log}"
    );
    assert_eq!(
        log.matches("\"method\":\"session/load\"").count(),
        1,
        "expected respawn to issue exactly one session/load using the persisted id, log={log}"
    );
}

#[test]
fn acp_send_selects_session_load_with_configured_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
    assert!(log.contains("\"sessionId\":\"sess-abc\""), "log={log}");
}

#[test]
fn acp_load_failure_does_not_fallback_to_session_new() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_load: true,
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    assert_acp_delivery_unavailable(&config_root, &temporary.path().join("tmux.sock"));
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
}

#[test]
fn acp_new_failure_returns_runtime_stage_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_new: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    assert_acp_delivery_unavailable(&config_root, &temporary.path().join("tmux.sock"));
}

// Regression guard for the give-up policy being stage-agnostic. A worker whose
// session/new bootstrap deterministically fails must make the respawn monitor
// give up after a bounded number of identical failures rather than respawn
// forever. Before the policy counted repeated failures of any bootstrap stage
// (it previously counted only `initialize`), a session/new failure reset the
// give-up counter on every attempt, so the monitor looped indefinitely; that
// never-settling churn is what made the sibling settle-unavailable tests flake
// under load, since their watch await had to win a race against the brief
// pre-churn Unavailable transient.
//
// The bounded-vs-unbounded respawn is asserted by the census of logged
// session/new requests: it freezes once the monitor gives up, and grows without
// bound while it churns. Two spaced samples discriminate the two behaviors
// without depending on catching any single readiness transition (the initial
// bootstrap publishes a real Unavailable, so state-based waits race it).
#[test]
fn acp_repeated_new_failure_gives_up_instead_of_respawning_forever() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_new: true,
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    // A delivery to the failed worker drives the respawn loop. Each respawn
    // attempt re-spawns the stub and re-issues session/new (nothing was
    // persisted, so the load-vs-new lifecycle stays on new), so the count of
    // logged session/new requests is a direct census of respawn attempts.
    let _ = dispatch_send_result(&config_root, &tmux_socket);

    // Let the respawn monitor run to its terminal decision. The stage-agnostic
    // give-up abandons the repeatedly-failing session/new after a bounded number
    // of identical failures, freezing the attempt count; the old initialize-only
    // policy reset its counter on every session/new failure and respawned
    // forever, re-issuing session/new on each backoff cycle (~10+/s at the test
    // backoff cap). Comparing two spaced samples discriminates the two without
    // depending on catching any single readiness transition.
    thread::sleep(Duration::from_secs(3));
    let attempts_after_settle =
        request_count_by_method(&read_request_log(&log_path), "session/new");
    thread::sleep(Duration::from_secs(1));
    let attempts_later = request_count_by_method(&read_request_log(&log_path), "session/new");

    assert_eq!(
        attempts_later, attempts_after_settle,
        "session/new bootstrap attempts kept growing ({attempts_after_settle} -> {attempts_later}); \
         respawn never gave up"
    );
}

#[test]
fn acp_missing_load_capability_returns_canonical_failure_code_and_details() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        load_capability: false,
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    assert_acp_delivery_unavailable(&config_root, &temporary.path().join("tmux.sock"));
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(!log.contains("\"method\":\"session/load\""), "log={log}");
}

#[test]
fn acp_missing_prompt_capability_returns_canonical_failure_code_and_details() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_capability: false,
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    assert_acp_delivery_unavailable(&config_root, &temporary.path().join("tmux.sock"));
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(!log.contains("\"method\":\"session/prompt\""), "log={log}");
}

#[test]
fn acp_initialize_failure_returns_canonical_runtime_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_initialize: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    assert_acp_delivery_unavailable(&config_root, &temporary.path().join("tmux.sock"));
}

#[test]
fn acp_prompt_failure_keeps_persistent_worker_available() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_prompt: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    // A JSON-RPC prompt error is a logical failure from a still-responsive
    // agent; the persistent worker stays available for subsequent prompts.
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(2)
        ),
        "worker_state did not stay available after a prompt error"
    );
}

#[test]
fn acp_disconnect_before_first_activity_engages_auto_respawn() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        disconnect_on_prompt: Some("before_activity".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    // Auto-respawn pulls the worker out of `unavailable` as soon as it
    // observes the ConnectionClosed transition; assert that the worker
    // transitions through the recovery path (or settles back at available)
    // rather than staying terminally unavailable.
    assert!(
        wait_for_any_worker_state(
            temporary.path(),
            "bravo",
            &["recovering", "available", "busy"],
            Duration::from_secs(3),
        ),
        "worker_state did not engage auto-respawn after disconnect"
    );
}

#[test]
fn acp_disconnect_after_first_activity_preserves_accepted_response() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        disconnect_on_prompt: Some("after_activity".to_string()),
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    assert_eq!(result.reason_code, None);
    assert!(
        wait_for_any_worker_state(
            temporary.path(),
            "bravo",
            &["recovering", "available", "busy"],
            Duration::from_secs(3),
        ),
        "worker_state did not engage auto-respawn after disconnect"
    );
}

fn wait_for_any_worker_state(
    root: &std::path::Path,
    target_session: &str,
    expected: &[&str],
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(state) = read_worker_state(root, target_session)
            && expected.iter().any(|candidate| state == *candidate)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn wait_for_log_match(log_path: &std::path::Path, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(contents) = fs::read_to_string(log_path)
            && contents.contains(needle)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn wait_for_request_count(log_path: &std::path::Path, method: &str, expected: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let requests = read_request_log(log_path);
        if request_count_by_method(&requests, method) >= expected || Instant::now() >= deadline {
            return requests;
        }
        thread::sleep(Duration::from_millis(20));
    }
}
