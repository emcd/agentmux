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
    // The bundle was named, so the fault is retained with its own cause rather
    // than flattened into a generic unassociated server: an operator who
    // mistyped a bundle has something to repair, and the code that says so is
    // the difference between repairing it and re-reading the configuration.
    assert_eq!(
        response["error"]["data"]["code"],
        Value::String("validation_unknown_bundle".to_string())
    );
    let message = response["error"]["data"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("missing"),
        "retained fault should name the unresolvable bundle: {message}"
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_retains_an_uncreatable_inscriptions_path_while_still_serving_the_protocol() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("outside");
    let config_root = root.join("config");
    let state_root = root.join("state");
    let inscriptions_root = root.join("inscriptions");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "party", &["alpha", "bravo"]);

    // A regular file where the inscriptions tree needs a directory: creating the
    // sink fails without any of the protocol machinery being involved.
    fs::write(inscriptions_root.join("bundles"), "not a directory")
        .expect("block the inscriptions tree");

    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--bundle-name",
            "party",
            "--session-name",
            "alpha",
            "--configuration-directory",
            config_root.to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
            "--inscriptions-directory",
            inscriptions_root.to_str().expect("utf8 inscriptions path"),
        ],
        &[],
    )
    .await;

    // Reaching here at all means `initialize` completed, so the tool inventory
    // was negotiated rather than erased.
    let help_response = harness.call_tool(2, "help", Map::new()).await;
    assert_eq!(decode_tool_payload(&help_response)["namespace"], "agentmux");

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(3, "send", arguments).await;
    // Retained, not merely survived: a fault written only to stderr reaches
    // nobody, and this class includes a foreign-owned sink, where running
    // unlogged is the condition that must not pass quietly.
    assert_eq!(
        response["error"]["data"]["code"],
        Value::String("runtime_startup_failed".to_string())
    );
    let message = response["error"]["data"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("bundles"),
        "retained fault should name the path it could not create: {message}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_reports_argument_faults_ahead_of_a_retained_startup_fault() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("outside");
    let config_root = root.join("config");
    let state_root = root.join("state");
    let inscriptions_root = root.join("inscriptions");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "party", &["alpha", "bravo"]);
    fs::write(inscriptions_root.join("bundles"), "not a directory")
        .expect("block the inscriptions tree");

    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--bundle-name",
            "party",
            "--session-name",
            "alpha",
            "--configuration-directory",
            config_root.to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
            "--inscriptions-directory",
            inscriptions_root.to_str().expect("utf8 inscriptions path"),
        ],
        &[],
    )
    .await;

    // A request is validated on its own terms before the readiness guard is
    // consulted. Answering a malformed call with the retained fault would tell
    // an operator to repair the wrong thing, and hide the argument error until
    // the unrelated fault is cleared.
    let malformed: [(&str, Value); 4] = [
        ("updown", json!({"command": "sideways"})),
        ("new", json!({"command": "bogus"})),
        ("change", json!({"command": "bogus"})),
        ("choose", json!({})),
    ];
    let mut request_id = 2;
    for (tool, arguments) in malformed {
        let arguments = arguments.as_object().expect("arguments object").clone();
        let response = harness.call_tool(request_id, tool, arguments).await;
        assert_eq!(
            response["error"]["data"]["code"],
            Value::String("validation_invalid_params".to_string()),
            "{tool} must report its own argument fault, not the retained startup fault"
        );
        request_id += 1;
    }

    // The retained fault still reaches a well-formed call.
    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("down".to_string()));
    let response = harness.call_tool(request_id, "updown", arguments).await;
    assert_eq!(
        response["error"]["data"]["code"],
        Value::String("runtime_startup_failed".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_refuses_an_invalid_session_selector_rather_than_binding_the_directory_match() {
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
        Arc::new(|_request| {
            json!({
                "kind": "error",
                "error": {
                    "code": "internal_unexpected_failure",
                    "message": "the relay should never be reached",
                },
            })
        }),
    );

    // The working directory matches `coordinator`, so the deleted fallthrough
    // would have authenticated as that member despite the operator naming
    // something else. A mistyped selector must not silently become a different
    // identity -- that is the inference this ladder exists to remove.
    let git_environment = hook_git_environment();
    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--default-bundle",
            "master",
            "--session-name",
            "coordinatorr",
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
    assert_eq!(
        response["error"]["data"]["code"],
        Value::String("validation_unknown_sender".to_string())
    );
    assert!(
        relay.requests_for_operation("send").is_empty(),
        "a refused identity must not reach the relay at all"
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_retains_absent_configuration_root_fault_until_tool_time() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("outside");
    let state_root = root.join("state");
    fs::create_dir_all(&workspace).expect("create workspace");

    // A named configuration root that does not exist is a fault, not a reason
    // to scaffold one -- and not a reason to erase the tool surface either.
    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--configuration-directory",
            root.join("nowhere").to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
        ],
        &[],
    )
    .await;

    // The surface negotiated at initialize stays intact.
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
    let error = &response["error"]["data"];
    assert_eq!(
        error["code"],
        Value::String("validation_configuration_root_absent".to_string()),
        "the retained fault must name the actual defect, not a generic one: {response}"
    );
    assert_eq!(
        error["details"]["reason"],
        Value::String("startup_fault".to_string())
    );

    // Snapshotted: a second call reports the same cause rather than
    // re-deriving it or drifting to a different one.
    let mut second = Map::new();
    second.insert("message".to_string(), Value::String("again".to_string()));
    second.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("alpha".to_string())]),
    );
    second.insert("broadcast".to_string(), Value::Bool(false));
    let repeat = harness.call_tool(4, "send", second).await;
    assert_eq!(repeat["error"]["data"]["code"], error["code"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_reports_malformed_request_on_its_own_terms_not_the_retained_fault() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("outside");
    let state_root = root.join("state");
    fs::create_dir_all(&workspace).expect("create workspace");

    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--configuration-directory",
            root.join("nowhere").to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
        ],
        &[],
    )
    .await;

    // A malformed call must report its own defect. Reporting the retained
    // startup fault instead would send the caller to fix the wrong thing.
    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert("targets".to_string(), Value::Array(Vec::new()));
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;
    let error = &response["error"]["data"];
    assert_ne!(
        error["details"]["reason"],
        Value::String("startup_fault".to_string()),
        "a malformed request must not be answered with the retained fault: {response}"
    );
    assert_ne!(
        error["code"],
        Value::String("validation_configuration_root_absent".to_string()),
        "a malformed request must report its own defect: {response}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_unknown_flag_does_not_erase_the_tool_surface() {
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
            "--not-a-real-flag",
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
        response["error"]["data"]["details"]["reason"],
        Value::String("startup_fault".to_string()),
        "an argument fault must be retained, not fatal: {response}"
    );
}
