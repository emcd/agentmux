//! Bootstrap scenarios: concurrent bootstrap races for the relay socket,
//! stale-socket removal, MCP startup without an active bundle context,
//! explicit unknown-bundle startup failure, association discovery from a
//! non-git cwd, directory fallback when the auto-sender is not a
//! configured member, and the debug-build repository-root socket override.

use std::{
    fs,
    os::unix::net::UnixListener,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use agentmux::runtime::{
    bootstrap::{BootstrapOptions, bootstrap_relay},
    paths::{RelayRuntimePaths, ensure_relay_runtime_directory},
};
use serde_json::{Map, Value, json};
use tempfile::TempDir;

use super::mocks::{
    FakeRelay, McpHarness, decode_tool_payload, hook_git_environment, write_bundle_configuration,
    write_bundle_configuration_with_directories,
};

#[test]
fn concurrent_bootstrap_spawns_single_relay() {
    const CLIENTS: usize = 4;

    let temporary = TempDir::new().expect("temporary");
    let paths = RelayRuntimePaths::resolve(temporary.path());
    ensure_relay_runtime_directory(&paths).expect("runtime directory");

    let spawn_count = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(CLIENTS));
    let listener = Arc::new(Mutex::new(None::<UnixListener>));
    let options = BootstrapOptions {
        auto_start_relay: true,
        startup_timeout: Duration::from_secs(2),
    };

    let mut handles = Vec::new();
    for _ in 0..CLIENTS {
        let paths = paths.clone();
        let spawn_count = Arc::clone(&spawn_count);
        let barrier = Arc::clone(&barrier);
        let listener = Arc::clone(&listener);
        handles.push(thread::spawn(move || {
            barrier.wait();
            bootstrap_relay(&paths, options, || {
                if spawn_count.fetch_add(1, Ordering::SeqCst) == 0 {
                    let bound =
                        UnixListener::bind(&paths.relay_socket).expect("bind relay listener");
                    *listener.lock().expect("listener lock") = Some(bound);
                    fs::write(&paths.relay_ready_sentinel, b"")
                        .expect("write relay ready sentinel");
                }
                Ok(())
            })
            .map(|_| ())
        }));
    }

    for handle in handles {
        handle
            .join()
            .expect("thread join")
            .expect("bootstrap should succeed");
    }

    assert_eq!(spawn_count.load(Ordering::SeqCst), 1);
    drop(listener.lock().expect("listener lock").take());
}

#[test]
fn bootstrap_removes_stale_socket_before_spawn() {
    let temporary = TempDir::new().expect("temporary");
    let paths = RelayRuntimePaths::resolve(temporary.path());
    ensure_relay_runtime_directory(&paths).expect("runtime directory");
    fs::write(&paths.relay_socket, "stale").expect("write stale file");

    let options = BootstrapOptions {
        auto_start_relay: true,
        startup_timeout: Duration::from_secs(2),
    };
    let mut listener = None;

    let report = bootstrap_relay(&paths, options, || {
        assert!(
            !paths.relay_socket.exists(),
            "stale socket should be removed"
        );
        listener = Some(UnixListener::bind(&paths.relay_socket).expect("bind listener"));
        fs::write(&paths.relay_ready_sentinel, b"").expect("write relay ready sentinel");
        Ok(())
    })
    .expect("bootstrap should succeed");

    assert!(report.spawned_relay);
    drop(listener.take());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_initializes_without_active_bundle_context() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("outside");
    let config_root = root.join("config");
    let state_root = root.join("state");
    fs::create_dir_all(&workspace).expect("create workspace");
    write_bundle_configuration(&config_root, "party", &["alpha"]);

    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--configuration-directory",
            config_root.to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
        ],
        &[],
    )
    .await;

    let help_response = harness.call_tool(2, "help", Map::new()).await;
    let help_payload = decode_tool_payload(&help_response);
    assert_eq!(help_payload["namespace"], "agentmux");

    let mut send_arguments = Map::new();
    send_arguments.insert("message".to_string(), Value::String("hello".to_string()));
    send_arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("alpha".to_string())]),
    );
    send_arguments.insert("broadcast".to_string(), Value::Bool(false));
    let send_response = harness.call_tool(3, "send", send_arguments).await;
    assert_eq!(
        send_response["error"]["data"]["code"],
        Value::String("validation_unassociated_server".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_unknown_bundle_starts_green_and_reports_at_tool_time() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("outside");
    let config_root = root.join("config");
    let state_root = root.join("state");
    fs::create_dir_all(&workspace).expect("create workspace");
    write_bundle_configuration(&config_root, "party", &["alpha"]);

    // Failing startup would erase the advertised tool inventory, which some
    // harnesses never recover from, and bury the cause in a log the agent does
    // not read. The server starts and reports the fault where it can be acted on.
    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--bundle-name",
            "missing",
            "--configuration-directory",
            config_root.to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
        ],
        &[],
    )
    .await;

    let help_response = harness.call_tool(2, "help", Map::new()).await;
    assert_eq!(decode_tool_payload(&help_response)["namespace"], "agentmux");

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("alpha".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(3, "send", arguments).await;
    assert_eq!(
        response["error"]["data"]["code"],
        Value::String("validation_unassociated_server".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_associates_from_the_injected_bring_up_environment() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("relay");
    let config_root = root.join("config");
    let state_root = root.join("state");
    fs::create_dir_all(&workspace).expect("create workspace");
    write_bundle_configuration(&config_root, "relay", &["relay", "bravo"]);

    let relay_socket = state_root.join("relay.sock");
    let relay = FakeRelay::start(
        relay_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "bundle_name": "relay",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "results": [{
                        "target_session": "bravo",
                        "message_id": "msg-1",
                        "outcome": "delivered",
                    }],
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );

    // The channel through which bring-up states the identity it is starting,
    // end to end: configuration load stamps it, the transport applies it, and
    // this subprocess consumes it instead of inferring one from the filesystem.
    let mut environment = hook_git_environment();
    environment.push(("AGENTMUX_BUNDLE".to_string(), "relay".to_string()));
    environment.push(("AGENTMUX_SESSION".to_string(), "relay".to_string()));
    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--configuration-directory",
            config_root.to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
        ],
        &environment,
    )
    .await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["requester_session"], "relay");

    let send_requests = relay.requests_for_operation("send");
    assert_eq!(send_requests.len(), 1);
    assert_eq!(send_requests[0]["requester_session"], "relay");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_resolves_session_by_matching_declared_member_directories() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("master");
    let other = root.join("other");
    let config_root = root.join("config");
    let state_root = root.join("state");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(&other).expect("create other");
    write_bundle_configuration_with_directories(
        &config_root,
        "master",
        &[("coordinator", &workspace), ("bravo", &other)],
    );

    let relay_socket = state_root.join("relay.sock");
    let relay = FakeRelay::start(
        relay_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "bundle_name": "master",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "results": [{
                        "target_session": "bravo",
                        "message_id": "msg-1",
                        "outcome": "delivered",
                    }],
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );

    // Bundle from the default tier, session from nothing at all: the working
    // directory is matched against the directories the bundle file already
    // declares. That is declarative rather than inferential -- unlike the
    // deleted filesystem guess, it cannot produce a plausible wrong answer.
    let git_environment = hook_git_environment();
    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--default-bundle",
            "master",
            "--configuration-directory",
            config_root.to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
        ],
        &git_environment,
    )
    .await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["requester_session"], "coordinator");

    let send_requests = relay.requests_for_operation("send");
    assert_eq!(send_requests.len(), 1);
    assert_eq!(send_requests[0]["requester_session"], "coordinator");
}

#[cfg(debug_assertions)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_uses_repository_root_debug_state_override() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("workspace");
    let repository_root = root.join("repository");
    let config_root = root.join("config");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(&repository_root).expect("create repository root");
    write_bundle_configuration(&config_root, "party", &["alpha", "bravo"]);

    let relay_socket = repository_root
        .join(".auxiliary/state/agentmux")
        .join("relay.sock");
    let relay = FakeRelay::start(
        relay_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "party",
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "principals": [{"id": "bravo", "transport": "tmux", "ready": true}],
                    },
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );

    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--bundle-name",
            "party",
            "--session-name",
            "alpha",
            "--configuration-directory",
            config_root.to_str().expect("utf8 config path"),
            "--repository-root",
            repository_root.to_str().expect("utf8 repository path"),
        ],
        &[],
    )
    .await;

    let response = harness
        .call_tool(
            2,
            "list",
            Map::from_iter([
                (
                    "command".to_string(),
                    Value::String("principals".to_string()),
                ),
                ("args".to_string(), Value::Object(Map::new())),
            ]),
        )
        .await;
    let payload = decode_tool_payload(&response);
    let bundles = payload["bundles"]
        .as_array()
        .expect("bundles must be array");
    assert_eq!(bundles[0]["id"], "party");
    // Two list calls: the configured bundle and the relay-wide GLOBAL fetch.
    // The local FakeRelay returns the same canned payload to both; this test
    // verifies only that the debug-override socket was reachable.
    assert_eq!(relay.requests_for_operation("list").len(), 2);
}
