use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use agentmux::{
    relay::{
        ChatDeliveryMode, ChatOutcome, RelayRequest, RelayResponse, handle_request,
        serve_connection,
    },
    runtime::paths::BundleRuntimePaths,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

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

fn hello_payload(bundle_name: &str, session_id: &str) -> Value {
    json!({
        "frame": "hello",
        "schema_version": "1",
        "bundle_name": bundle_name,
        "session_id": session_id,
        "client_class": "ui"
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

    send_json(&mut ui_client, hello_payload(bundle_name.as_str(), "bravo"));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-1".to_string()),
            sender_session: "alpha".to_string(),
            message: "hello ui".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: None,
            quiescence_timeout_ms: Some(500),
            acp_turn_timeout_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.tmux_socket,
    )
    .expect("chat response");
    let RelayResponse::Chat { results, .. } = response else {
        panic!("expected chat response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Delivered);

    let incoming_event = read_json(&mut reader);
    assert_eq!(incoming_event["frame"], "event");
    assert_eq!(incoming_event["event"]["event_type"], "incoming_message");
    assert_eq!(incoming_event["event"]["bundle_name"], bundle_name);
    assert_eq!(incoming_event["event"]["target_session"], "bravo");
    assert_eq!(
        incoming_event["event"]["payload"]["sender_session"],
        "alpha"
    );

    let outcome_event = read_json(&mut reader);
    assert_eq!(outcome_event["frame"], "event");
    assert_eq!(outcome_event["event"]["event_type"], "delivery_outcome");
    assert_eq!(outcome_event["event"]["payload"]["outcome"], "success");

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
        hello_payload(bundle_name.as_str(), "bravo"),
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
            hello_payload(reconnect_bundle.as_str(), "bravo"),
        );
        let ack = read_json(&mut reconnect_reader);
        let incoming = read_json(&mut reconnect_reader);
        let outcome = read_json(&mut reconnect_reader);
        reconnect_client
            .shutdown(std::net::Shutdown::Both)
            .expect("shutdown reconnect stream");
        (ack, incoming, outcome)
    });

    let start = Instant::now();
    let response = handle_request(
        RelayRequest::Chat {
            request_id: Some("req-2".to_string()),
            sender_session: "alpha".to_string(),
            message: "wait for reconnect".to_string(),
            targets: vec!["bravo".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: None,
            quiescence_timeout_ms: Some(1_000),
            acp_turn_timeout_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.tmux_socket,
    )
    .expect("chat response");
    assert!(
        start.elapsed() >= Duration::from_millis(120),
        "chat should wait for reconnect before delivery"
    );

    let RelayResponse::Chat { results, .. } = response else {
        panic!("expected chat response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Delivered);

    let (ack, incoming, outcome) = reconnect_thread.join().expect("join reconnect thread");
    assert_eq!(ack["frame"], "hello_ack");
    assert_eq!(incoming["event"]["event_type"], "incoming_message");
    assert_eq!(outcome["event"]["event_type"], "delivery_outcome");
    reconnect_handle.join().expect("join reconnect server");
}
