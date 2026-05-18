use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use agentmux::{
    relay::{
        ChatOutcome, ChatStatus, RelayRequest, RelayResponse, handle_request, serve_connection,
    },
    runtime::paths::BundleRuntimePaths,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

fn dispatch_request(
    request: RelayRequest,
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(request, configuration_root, bundle_name, runtime_directory)
}

fn write_bundle_configuration(temporary: &TempDir, bundle_name: &str) -> PathBuf {
    let configuration_root = temporary.path().join("config");
    let bundles_directory = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles_directory).expect("create bundles directory");
    std::fs::write(
        configuration_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders configuration");
    std::fs::write(
        configuration_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "self"
send = "all:home"
"#,
    )
    .expect("write policies configuration");
    std::fs::write(
        configuration_root.join("users.toml"),
        r#"
default-bundle = "example"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    )
    .expect("write users configuration");
    std::fs::write(
        bundles_directory.join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "shell"
"#,
    )
    .expect("write bundle configuration");
    configuration_root
}

fn spawn_relay_stream(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (mut server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let paths = bundle_paths.clone();
    let handle = thread::spawn(move || {
        serve_connection(&mut server_stream, &root, &paths).expect("serve connection");
    });
    (client_stream, handle)
}

fn send_json(stream: &mut UnixStream, payload: Value) {
    let encoded = serde_json::to_string(&payload).expect("encode payload");
    stream
        .write_all(format!("{encoded}\n").as_bytes())
        .expect("write payload");
    stream.flush().expect("flush payload");
}

fn read_json(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    let read = reader.read_line(&mut line).expect("read payload");
    assert!(read > 0, "expected payload");
    serde_json::from_str::<Value>(line.trim_end()).expect("decode payload")
}

fn read_json_with_timeout(reader: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(read) => {
            if read == 0 {
                return None;
            }
            Some(serde_json::from_str::<Value>(line.trim_end()).expect("decode payload"))
        }
        Err(source) if matches!(source.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => None,
        Err(source) => panic!("read payload: {source}"),
    }
}

fn hello_payload(bundle_name: &str, session_id: &str) -> Value {
    json!({
        "frame": "hello",
        "schema_version": "1",
        "bundle_name": bundle_name,
        "session_id": session_id,
    })
}

#[test]
fn relay_chat_routes_to_connected_ui_stream_with_event_frames() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let (mut ui_client, ui_handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let read_stream = ui_client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut ui_client,
        hello_payload(bundle_name.as_str(), "user@GLOBAL"),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-1".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello ui".to_string(),
            targets: vec!["user@GLOBAL".to_string()],
            broadcast: false,
            quiet_window_ms: None,
            quiescence_timeout_ms: Some(500),
            acp_turn_timeout_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("chat response");
    let RelayResponse::Chat { results, .. } = response else {
        panic!("expected chat response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Queued);

    let first_event = read_json(&mut reader);
    let second_event = read_json(&mut reader);
    let third_event = read_json(&mut reader);
    let events = [&first_event, &second_event, &third_event];
    let incoming_event = events
        .iter()
        .find(|value| value["event"]["event_type"] == "incoming_message")
        .expect("incoming event");
    assert_eq!(incoming_event["event"]["bundle_name"], bundle_name);
    assert_eq!(incoming_event["event"]["target_session"], "user@GLOBAL");
    assert_eq!(
        incoming_event["event"]["payload"]["sender_session"],
        format!("alpha@{bundle_name}")
    );

    let routed_event = events
        .iter()
        .find(|value| {
            value["event"]["event_type"] == "delivery_outcome"
                && value["event"]["payload"]["phase"] == "routed"
        })
        .expect("routed delivery outcome");
    assert!(routed_event["event"]["payload"]["outcome"].is_null());
    assert_eq!(
        routed_event["event"]["payload"]["message_id"],
        results[0].message_id
    );

    let delivered_event = events
        .iter()
        .find(|value| {
            value["event"]["event_type"] == "delivery_outcome"
                && value["event"]["payload"]["phase"] == "delivered"
        })
        .expect("delivered outcome");
    assert_eq!(delivered_event["event"]["payload"]["outcome"], "success");
    assert_eq!(
        delivered_event["event"]["payload"]["message_id"],
        results[0].message_id
    );

    ui_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown ui stream");
    ui_handle.join().expect("join relay stream");
}

#[test]
fn relay_chat_waits_for_ui_reconnect_before_delivery() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");

    let (mut first_client, first_handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let first_reader_stream = first_client.try_clone().expect("clone stream");
    let mut first_reader = BufReader::new(first_reader_stream);
    send_json(
        &mut first_client,
        hello_payload(bundle_name.as_str(), "user@GLOBAL"),
    );
    let _ = read_json(&mut first_reader);
    first_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown initial stream");
    first_handle.join().expect("join initial stream");

    let (mut reconnect_client, reconnect_handle) =
        spawn_relay_stream(&configuration_root, &bundle_paths);
    let reconnect_reader_stream = reconnect_client
        .try_clone()
        .expect("clone reconnect stream");
    let mut reconnect_reader = BufReader::new(reconnect_reader_stream);
    let reconnect_bundle = bundle_name.clone();
    let reconnect_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        send_json(
            &mut reconnect_client,
            hello_payload(reconnect_bundle.as_str(), "user@GLOBAL"),
        );
        let ack = read_json(&mut reconnect_reader);
        let first_event = read_json(&mut reconnect_reader);
        let second_event = read_json(&mut reconnect_reader);
        let third_event = read_json(&mut reconnect_reader);
        reconnect_client
            .shutdown(std::net::Shutdown::Both)
            .expect("shutdown reconnect stream");
        (ack, first_event, second_event, third_event)
    });

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-2".to_string()),
            sender_session: "alpha".to_string(),
            message: "wait for reconnect".to_string(),
            targets: vec!["user@GLOBAL".to_string()],
            broadcast: false,
            quiet_window_ms: None,
            quiescence_timeout_ms: Some(1_000),
            acp_turn_timeout_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("chat response");

    let RelayResponse::Chat { results, .. } = response else {
        panic!("expected chat response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Queued);

    // Async dispatch returns immediately; the reconnect thread connects at
    // +150ms and still receives the terminal delivered/success event, which
    // proves the background worker held delivery until the UI reconnected.
    let (ack, first_event, second_event, third_event) =
        reconnect_thread.join().expect("join reconnect thread");
    let events = [&first_event, &second_event, &third_event];
    assert_eq!(ack["frame"], "hello_ack");
    assert!(
        events
            .iter()
            .any(|value| value["event"]["event_type"] == "incoming_message")
    );
    assert!(events.iter().any(|value| {
        value["event"]["event_type"] == "delivery_outcome"
            && value["event"]["payload"]["phase"] == "routed"
    }));
    assert!(events.iter().any(|value| {
        value["event"]["event_type"] == "delivery_outcome"
            && value["event"]["payload"]["phase"] == "delivered"
            && value["event"]["payload"]["outcome"] == "success"
    }));
    reconnect_handle.join().expect("join reconnect server");
}

#[test]
fn relay_async_chat_emits_terminal_delivery_outcome_to_sender_ui_stream() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");

    let (mut sender_client, sender_handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let sender_read_stream = sender_client.try_clone().expect("clone sender stream");
    sender_read_stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set sender read timeout");
    let mut sender_reader = BufReader::new(sender_read_stream);
    send_json(
        &mut sender_client,
        hello_payload(bundle_name.as_str(), "user@GLOBAL"),
    );
    let sender_ack = read_json(&mut sender_reader);
    assert_eq!(sender_ack["frame"], "hello_ack");

    let response = dispatch_request(
        RelayRequest::Chat {
            request_id: Some("req-async-sender".to_string()),
            sender_session: "user@GLOBAL".to_string(),
            message: "verify sender completion stream".to_string(),
            targets: vec!["alpha".to_string()],
            broadcast: false,
            quiet_window_ms: None,
            quiescence_timeout_ms: Some(500),
            acp_turn_timeout_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("chat response");
    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Accepted);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Queued);
    let expected_message_id = results[0].message_id.clone();

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut observed_sender_outcome = None::<Value>;
    while Instant::now() < deadline {
        if let Some(frame) = read_json_with_timeout(&mut sender_reader)
            && frame["frame"] == "event"
            && frame["event"]["event_type"] == "delivery_outcome"
            && frame["event"]["payload"]["message_id"] == expected_message_id
        {
            let phase = frame["event"]["payload"]["phase"]
                .as_str()
                .unwrap_or_default();
            let outcome = frame["event"]["payload"]["outcome"]
                .as_str()
                .unwrap_or_default();
            if (phase == "delivered" && outcome == "success")
                || (phase == "failed" && (outcome == "timeout" || outcome == "failed"))
            {
                observed_sender_outcome = Some(frame);
                break;
            }
        }
    }
    assert!(
        observed_sender_outcome.is_some(),
        "expected sender stream to receive terminal delivery_outcome for queued async message"
    );

    sender_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown sender stream");
    sender_handle.join().expect("join sender relay stream");
}

// ---------------------------------------------------------------------------
// Grant-authorized permission list and submitter gating
// ---------------------------------------------------------------------------

fn write_operator_bundle_configuration(temporary: &TempDir, bundle_name: &str) -> PathBuf {
    let configuration_root = temporary.path().join("config");
    let bundles_directory = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles_directory).expect("create bundles directory");
    std::fs::write(
        configuration_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders configuration");
    std::fs::write(
        configuration_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "self"
send = "all:home"
grant = "none"

[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "all:home"
look = "all:home"
send = "all:home"
grant = "all:home"
"#,
    )
    .expect("write policies configuration");
    std::fs::write(
        bundles_directory.join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
policy = "operator"
coder = "shell"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "shell"
"#,
    )
    .expect("write bundle configuration");
    configuration_root
}

#[test]
fn relay_accepts_hello_for_configured_bundle_member() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_operator_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let (mut client, handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);

    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");
    assert_eq!(hello_ack["session_id"], "alpha");

    client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown stream");
    handle.join().expect("join relay stream");
}

#[test]
fn relay_permission_list_succeeds_for_grant_authorized_principal() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_operator_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    std::fs::create_dir_all(&bundle_paths.runtime_directory).expect("create runtime directory");

    let (mut client, handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);

    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {"operation": "permission_list"},
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], request_id);
    assert_eq!(response["response"]["kind"], "permission_list");
    assert_eq!(response["response"]["bundle_name"], bundle_name);
    let entries = response["response"]["pending_requests"]
        .as_array()
        .expect("pending_requests array");
    assert!(entries.is_empty(), "no requests have been queued yet");

    client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown stream");
    handle.join().expect("join relay stream");
}

#[test]
fn relay_permission_resolve_rejects_submitter_without_grant() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let (mut client, handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);

    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled",
            },
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );

    client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown stream");
    handle.join().expect("join relay stream");
}
