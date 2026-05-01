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

fn write_tui_configuration(configuration_root: &Path, policy: &str) {
    std::fs::write(
        configuration_root.join("tui.toml"),
        format!(
            r#"
default-bundle = "party"
default-session = "user"

[[sessions]]
id = "user"
policy = "{policy}"
"#
        ),
    )
    .expect("write tui configuration");
}

fn write_policies_with_grant(configuration_root: &Path, grant: &str) {
    std::fs::write(
        configuration_root.join("policies.toml"),
        format!(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
grant = "{grant}"
list = "all:home"
look = "self"
send = "all:home"
"#
        ),
    )
    .expect("write policies configuration");
}

fn seed_permission_queue(runtime_directory: &Path, permission_request_id: &str, option_id: &str) {
    std::fs::create_dir_all(runtime_directory).expect("create runtime directory");
    std::fs::write(
        runtime_directory.join("permission_queue.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "next_sequence": 2,
  "pending": [
    {{
      "permission_request_id": "{permission_request_id}",
      "message_id": "msg-1",
      "target_session": "alpha",
      "requested_kind": "execute",
      "requested_details": {{
        "tool_call_title": "Run command",
        "options": [
          {{
            "option_id": "{option_id}",
            "name": "Allow once",
            "kind": "allow_once"
          }}
        ]
      }},
      "enqueued_at": "2026-01-01T00:00:00Z",
      "enqueued_at_ms": 12345,
      "sequence": 1
    }}
  ]
}}"#
        ),
    )
    .expect("write seeded permission queue");
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

fn shutdown_stream(stream: &UnixStream, context: &str) {
    match stream.shutdown(std::net::Shutdown::Both) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotConnected => {}
        Err(source) => panic!("{context}: {source:?}"),
    }
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

    shutdown_stream(&client_stream, "shutdown client stream");
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
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "list");
    assert_eq!(response["response"]["bundle"]["id"], bundle_name);

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn duplicate_live_hello_claim_is_rejected_with_identity_conflict() {
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
    let second_response = read_json(&mut second_reader);
    assert_eq!(second_response["frame"], "response");
    assert_eq!(second_response["response"]["kind"], "error");
    assert_eq!(
        second_response["response"]["error"]["code"],
        "runtime_identity_claim_conflict"
    );
    assert_eq!(
        second_response["response"]["error"]["details"]["bundle_name"],
        bundle_name
    );
    assert_eq!(
        second_response["response"]["error"]["details"]["session_id"],
        "alpha"
    );
    assert_eq!(
        second_response["response"]["error"]["details"]["reason"],
        "existing identity owner is still live"
    );

    send_json(
        &mut first_client,
        json!({
            "frame": "request",
            "request": {"operation": "list", "sender_session": "alpha"}
        }),
    );
    let first_response = read_json(&mut first_reader);
    assert_eq!(first_response["frame"], "response");
    assert_eq!(first_response["response"]["kind"], "list");

    shutdown_stream(&first_client, "shutdown first client");
    shutdown_stream(&second_client, "shutdown second client");
    first_handle.join().expect("join first relay thread");
    second_handle.join().expect("join second relay thread");
}

#[test]
fn hello_claim_is_accepted_after_prior_owner_disconnects() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_reconnect_after_disconnect";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut first_client, first_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let first_read_stream = first_client.try_clone().expect("clone first stream");
    let mut first_reader = BufReader::new(first_read_stream);

    send_json(
        &mut first_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "bundle_name": bundle_name,
            "session_id": "alpha",
            "client_class": "agent"
        }),
    );
    let first_ack = read_json(&mut first_reader);
    assert_eq!(first_ack["frame"], "hello_ack");
    shutdown_stream(&first_client, "shutdown first client");
    first_handle.join().expect("join first relay thread");

    let (mut second_client, second_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let second_read_stream = second_client.try_clone().expect("clone second stream");
    let mut second_reader = BufReader::new(second_read_stream);
    send_json(
        &mut second_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "bundle_name": bundle_name,
            "session_id": "alpha",
            "client_class": "agent"
        }),
    );
    let second_ack = read_json(&mut second_reader);
    assert_eq!(second_ack["frame"], "hello_ack");

    shutdown_stream(&second_client, "shutdown second client");
    second_handle.join().expect("join second relay thread");
}

#[test]
fn permission_decision_rejects_non_ui_stream_submitter() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_non_ui";
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

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled"
            }
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_client_class_for_action"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_rejects_payload_actor_spoof_field() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_actor_spoof";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
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
            "session_id": "user",
            "client_class": "ui"
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled",
                "ui_session_id": "spoofed"
            }
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_denial_uses_grant_capability() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_grant_capability";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
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
            "session_id": "user",
            "client_class": "ui"
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled"
            }
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "grant"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_rejects_empty_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_empty_option";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
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
            "session_id": "user",
            "client_class": "ui"
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "selected",
                "option_id": "   "
            }
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "option_id"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_rejects_selected_without_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_selected_missing_option";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
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
            "session_id": "user",
            "client_class": "ui"
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "selected"
            }
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "option_id"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_rejects_cancelled_with_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_cancelled_with_option";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
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
            "session_id": "user",
            "client_class": "ui"
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled",
                "option_id": "allow-once"
            }
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "option_id"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_resolve_selected_rejects_unknown_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_unknown_option";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
    write_policies_with_grant(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue(
        &bundle_paths.runtime_directory,
        "perm-unknown-option",
        "allow-once",
    );
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
            "session_id": "user",
            "client_class": "ui"
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-unknown-option",
                "outcome": "selected",
                "option_id": "not-present"
            }
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "option_id"
    );
    assert_eq!(
        response["response"]["error"]["details"]["value"],
        "not-present"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}
