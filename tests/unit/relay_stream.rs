use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
};

use agentmux::{relay::serve_connection, runtime::paths::BundleRuntimePaths};
use serde_json::{Value, json};
use tempfile::TempDir;

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

fn spawn_relay_connection(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (mut server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let paths = bundle_paths.clone();
    let join_handle = thread::spawn(move || {
        serve_connection(&mut server_stream, &root, &paths).expect("serve connection")
    });
    (client_stream, join_handle)
}

fn send_json(stream: &mut UnixStream, payload: Value) {
    let encoded = serde_json::to_string(&payload).expect("encode payload");
    stream
        .write_all(format!("{encoded}\n").as_bytes())
        .expect("write frame");
    stream.flush().expect("flush frame");
}

fn read_json(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    let read = reader.read_line(&mut line).expect("read frame");
    assert!(read > 0, "expected frame");
    serde_json::from_str::<Value>(line.trim_end()).expect("decode frame")
}

#[test]
fn stream_request_before_hello_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_before_hello";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request": {"operation": "list", "sender_session": "alpha"}
        }),
    );
    let frame = read_json(&mut reader);
    assert_eq!(frame["frame"], "response");
    assert_eq!(frame["response"]["kind"], "error");
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_missing_hello"
    );

    client_stream
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn stream_hello_acknowledges_and_allows_request() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_allow_request";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "bundle_name": bundle_name,
            "session_id": "alpha",
            "client_class": "agent"
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");
    assert_eq!(hello_ack["bundle_name"], bundle_name);
    assert_eq!(hello_ack["session_id"], "alpha");
    assert_eq!(hello_ack["client_class"], "agent");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {"operation": "list", "sender_session": "alpha"}
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "list");
    assert_eq!(response["response"]["bundle_name"], bundle_name);

    client_stream
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn reconnecting_hello_replaces_prior_stream_identity_binding() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_reconnect";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut first_client, first_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let first_read_stream = first_client.try_clone().expect("clone first stream");
    let mut first_reader = BufReader::new(first_read_stream);

    let (mut second_client, second_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let second_read_stream = second_client.try_clone().expect("clone second stream");
    let mut second_reader = BufReader::new(second_read_stream);

    let hello_frame = json!({
        "frame": "hello",
        "schema_version": "1",
        "bundle_name": bundle_name,
        "session_id": "alpha",
        "client_class": "agent"
    });

    send_json(&mut first_client, hello_frame.clone());
    let first_ack = read_json(&mut first_reader);
    assert_eq!(first_ack["frame"], "hello_ack");

    send_json(&mut second_client, hello_frame);
    let second_ack = read_json(&mut second_reader);
    assert_eq!(second_ack["frame"], "hello_ack");

    send_json(
        &mut first_client,
        json!({
            "frame": "request",
            "request": {"operation": "list", "sender_session": "alpha"}
        }),
    );
    let stale_response = read_json(&mut first_reader);
    assert_eq!(stale_response["frame"], "response");
    assert_eq!(stale_response["response"]["kind"], "error");
    assert_eq!(
        stale_response["response"]["error"]["code"],
        "validation_stale_stream_binding"
    );

    first_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown first client");
    second_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown second client");
    first_handle.join().expect("join first relay thread");
    second_handle.join().expect("join second relay thread");
}
