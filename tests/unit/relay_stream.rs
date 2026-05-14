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
    seed_permission_queue_with_options(runtime_directory, &[(permission_request_id, option_id)]);
}

fn seed_permission_queue_with_options(runtime_directory: &Path, entries: &[(&str, &str)]) {
    std::fs::create_dir_all(runtime_directory).expect("create runtime directory");
    let pending: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(index, (permission_request_id, option_id))| {
            let sequence = (index as u64) + 1;
            json!({
                "permission_request_id": permission_request_id,
                "message_id": format!("msg-{sequence}"),
                "target_session": "alpha",
                "requested_kind": "execute",
                "requested_details": {
                    "tool_call_title": format!("Run command {sequence}"),
                    "options": [
                        {
                            "option_id": option_id,
                            "name": "Allow once",
                            "kind": "allow_once"
                        },
                        {
                            "option_id": format!("{option_id}-reject"),
                            "name": "Reject once",
                            "kind": "reject_once"
                        }
                    ],
                    "acp_request_id": 100 + sequence,
                    "raw": {"sessionId": "alpha"},
                },
                "enqueued_at": "2026-01-01T00:00:00Z",
                "enqueued_at_ms": 12345 + (sequence as i64),
                "sequence": sequence,
            })
        })
        .collect();
    let next_sequence = (entries.len() as u64) + 1;
    let state = json!({
        "schema_version": 1,
        "next_sequence": next_sequence,
        "pending": pending,
    });
    std::fs::write(
        runtime_directory.join("permission_queue.json"),
        serde_json::to_string_pretty(&state).expect("encode seeded queue"),
    )
    .expect("write seeded permission queue");
}

fn read_until_event_type(reader: &mut BufReader<UnixStream>, event_type: &str) -> Value {
    for _ in 0..32 {
        let frame = read_json(reader);
        if frame["frame"] == "event"
            && frame["event"]["event_type"] == Value::String(event_type.to_string())
        {
            return frame;
        }
    }
    panic!("did not observe event_type '{event_type}' within frame budget");
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
fn permission_snapshot_then_replay_carries_option_metadata() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_snapshot_options";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
    write_policies_with_grant(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue_with_options(
        &bundle_paths.runtime_directory,
        &[("perm-aaa", "allow-once"), ("perm-bbb", "allow-once")],
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

    let snapshot = read_until_event_type(&mut reader, "permission.snapshot");
    let snapshot_payload = &snapshot["event"]["payload"];
    assert_eq!(snapshot_payload["pending_count"], 2);
    assert_eq!(
        snapshot_payload["permission_request_ids"],
        json!(["perm-aaa", "perm-bbb"])
    );

    let first = read_until_event_type(&mut reader, "permission.requested");
    let first_payload = &first["event"]["payload"];
    assert_eq!(first_payload["permission_request_id"], "perm-aaa");
    assert_eq!(first_payload["target_session"], "alpha");
    let options = first_payload["requested_details"]["options"]
        .as_array()
        .expect("options array on permission.requested payload");
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["option_id"], "allow-once");
    assert_eq!(options[0]["name"], "Allow once");
    assert_eq!(options[0]["kind"], "allow_once");
    assert_eq!(options[1]["option_id"], "allow-once-reject");
    assert_eq!(options[1]["kind"], "reject_once");

    let second = read_until_event_type(&mut reader, "permission.requested");
    assert_eq!(
        second["event"]["payload"]["permission_request_id"],
        "perm-bbb"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_request_persists_across_authorized_ui_reconnect() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_persists";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
    write_policies_with_grant(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue(
        &bundle_paths.runtime_directory,
        "perm-persistent",
        "allow-once",
    );

    let hello_frame = json!({
        "frame": "hello",
        "schema_version": "1",
        "bundle_name": bundle_name,
        "session_id": "user",
        "client_class": "ui"
    });

    let (mut first_client, first_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let first_read = first_client.try_clone().expect("clone first stream");
    let mut first_reader = BufReader::new(first_read);
    send_json(&mut first_client, hello_frame.clone());
    let ack = read_json(&mut first_reader);
    assert_eq!(ack["frame"], "hello_ack");
    let snapshot = read_until_event_type(&mut first_reader, "permission.snapshot");
    assert_eq!(snapshot["event"]["payload"]["pending_count"], 1);
    let requested = read_until_event_type(&mut first_reader, "permission.requested");
    assert_eq!(
        requested["event"]["payload"]["permission_request_id"],
        "perm-persistent"
    );
    shutdown_stream(&first_client, "shutdown first client");
    first_handle.join().expect("join first relay thread");

    thread::sleep(std::time::Duration::from_millis(200));

    let (mut second_client, second_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let second_read = second_client.try_clone().expect("clone second stream");
    let mut second_reader = BufReader::new(second_read);
    send_json(&mut second_client, hello_frame);
    let ack = read_json(&mut second_reader);
    assert_eq!(ack["frame"], "hello_ack");
    let snapshot = read_until_event_type(&mut second_reader, "permission.snapshot");
    assert_eq!(snapshot["event"]["payload"]["pending_count"], 1);
    let requested = read_until_event_type(&mut second_reader, "permission.requested");
    assert_eq!(
        requested["event"]["payload"]["permission_request_id"],
        "perm-persistent"
    );

    shutdown_stream(&second_client, "shutdown second client");
    second_handle.join().expect("join second relay thread");
}

#[test]
fn permission_resolve_selected_emits_resolved_event_with_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_selected_emit";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
    write_policies_with_grant(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue(
        &bundle_paths.runtime_directory,
        "perm-selected",
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
    let ack = read_json(&mut reader);
    assert_eq!(ack["frame"], "hello_ack");
    let _snapshot = read_until_event_type(&mut reader, "permission.snapshot");
    let _requested = read_until_event_type(&mut reader, "permission.requested");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-resolve-selected",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-selected",
                "outcome": "selected",
                "option_id": "allow-once"
            }
        }),
    );

    let resolved = read_until_event_type(&mut reader, "permission.resolved");
    let payload = &resolved["event"]["payload"];
    assert_eq!(payload["permission_request_id"], "perm-selected");
    assert_eq!(payload["outcome"], "selected");
    assert_eq!(payload["reason_code"], Value::Null);
    assert_eq!(payload["decided_by"], "user");

    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-resolve-selected");
    assert_eq!(response["response"]["kind"], "permission_decision");
    assert_eq!(response["response"]["outcome"], "selected");
    assert_eq!(response["response"]["status"], "resolved");
    assert_eq!(
        response["response"]["permission_request_id"],
        "perm-selected"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_resolve_cancelled_emits_resolved_event_with_reason_code() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_cancelled_emit";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
    write_policies_with_grant(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue(
        &bundle_paths.runtime_directory,
        "perm-cancelled",
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
    let ack = read_json(&mut reader);
    assert_eq!(ack["frame"], "hello_ack");
    let _snapshot = read_until_event_type(&mut reader, "permission.snapshot");
    let _requested = read_until_event_type(&mut reader, "permission.requested");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-resolve-cancelled",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-cancelled",
                "outcome": "cancelled"
            }
        }),
    );

    let resolved = read_until_event_type(&mut reader, "permission.resolved");
    let payload = &resolved["event"]["payload"];
    assert_eq!(payload["permission_request_id"], "perm-cancelled");
    assert_eq!(payload["outcome"], "cancelled");
    assert_eq!(
        payload["reason_code"],
        "runtime_permission_request_cancelled"
    );
    assert_eq!(payload["decided_by"], "user");

    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-resolve-cancelled");
    assert_eq!(response["response"]["kind"], "permission_decision");
    assert_eq!(response["response"]["outcome"], "cancelled");
    assert_eq!(
        response["response"]["reason_code"],
        "runtime_permission_request_cancelled"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_max_pending_out_of_range_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_max_pending_invalid";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default");
    write_policies_with_grant(&configuration_root, "all:home");
    std::fs::write(
        configuration_root.join("relay.toml"),
        r#"
[relay.permission]
max-pending = 10000
"#,
    )
    .expect("write relay configuration");
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
    let ack = read_json(&mut reader);
    assert_eq!(ack["frame"], "hello_ack");
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_arguments"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "relay.permission.max-pending"
    );
    assert_eq!(response["response"]["error"]["details"]["value"], 10000);
    assert_eq!(response["response"]["error"]["details"]["maximum"], 4096);

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
