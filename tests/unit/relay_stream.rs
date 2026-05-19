use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{io::AsRawFd, net::UnixStream},
    path::{Path, PathBuf},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
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
        configuration_root.join("users.toml"),
        format!(
            r#"
default-bundle = "party"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "{policy}"

[sessions.ui]
"#
        ),
    )
    .expect("write users configuration");
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

// Reads the next frame, skipping any stray `hello_ack` frames. The relay emits
// a `hello_ack` into a live connection as a conflict liveness probe; raw test
// clients must tolerate it the way the production stream client does.
fn read_json_skipping_hello_ack(reader: &mut BufReader<UnixStream>) -> Value {
    loop {
        let frame = read_json(reader);
        if frame["frame"] != "hello_ack" {
            return frame;
        }
    }
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
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");
    assert_eq!(hello_ack["bundle_name"], bundle_name);
    assert_eq!(hello_ack["session_id"], "alpha");

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
    // The conflicting reconnect probed this live owner with a `hello_ack`
    // frame; skip it the way the production client does before reading the
    // list response.
    let first_response = read_json_skipping_hello_ack(&mut first_reader);
    assert_eq!(first_response["frame"], "response");
    assert_eq!(first_response["response"]["kind"], "list");

    shutdown_stream(&first_client, "shutdown first client");
    shutdown_stream(&second_client, "shutdown second client");
    first_handle.join().expect("join first relay thread");
    second_handle.join().expect("join second relay thread");
}

#[test]
fn stale_identity_owner_is_evicted_when_reconnecting() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_stale_eviction";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let hello_frame = json!({
        "frame": "hello",
        "schema_version": "1",
        "bundle_name": bundle_name,
        "session_id": "alpha",
    });

    // Register the first connection, then shut down only its read half. The
    // write half stays open, so the relay's connection thread keeps blocking
    // on its read loop and never observes EOF: the registry entry stays "live"
    // even though the peer can no longer receive a delivery. This reproduces
    // the stale-owner state that a fast reconnect races against.
    let (mut first_client, first_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut first_reader = BufReader::new(first_client.try_clone().expect("clone first stream"));
    send_json(&mut first_client, hello_frame.clone());
    assert_eq!(read_json(&mut first_reader)["frame"], "hello_ack");
    first_client
        .shutdown(std::net::Shutdown::Read)
        .expect("shut down first client read half");

    // A reconnect with the same identity must probe the stale owner, find it
    // unreachable, evict it, and register — not return an identity conflict.
    let (mut second_client, second_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut second_reader = BufReader::new(second_client.try_clone().expect("clone second stream"));
    send_json(&mut second_client, hello_frame);
    let second_response = read_json(&mut second_reader);
    assert_eq!(
        second_response["frame"], "hello_ack",
        "reconnect should evict the stale owner, not conflict: {second_response}"
    );

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
        }),
    );
    let second_ack = read_json(&mut second_reader);
    assert_eq!(second_ack["frame"], "hello_ack");

    shutdown_stream(&second_client, "shutdown second client");
    second_handle.join().expect("join second relay thread");
}

#[test]
fn permission_decision_rejects_submitter_without_grant_capability() {
    // Permission decisioning is now gated on the `grant` policy capability
    // rather than a hello-asserted client class. The `alpha` bundle member
    // resolves to the default policy, which omits `grant`.
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_non_grant";
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
            "session_id": "user@GLOBAL",
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
            "session_id": "user@GLOBAL",
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
            "session_id": "user@GLOBAL",
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
            "session_id": "user@GLOBAL",
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
            "session_id": "user@GLOBAL",
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
            "session_id": "user@GLOBAL",
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
    assert_eq!(
        first_payload["target_session"],
        format!("alpha@{bundle_name}")
    );
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
        "session_id": "user@GLOBAL",
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
            "session_id": "user@GLOBAL",
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
    assert_eq!(payload["decided_by"], "user@GLOBAL");

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
            "session_id": "user@GLOBAL",
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
    assert_eq!(payload["decided_by"], "user@GLOBAL");

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
            "session_id": "user@GLOBAL",
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
            "session_id": "user@GLOBAL",
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

static WRITE_TIMEOUT_ENV: OnceLock<()> = OnceLock::new();

// Shrinks the relay-side write timeout so the stalled-client teardown is
// observable within a unit-test-friendly window. The override is process-wide;
// every other connection in this binary writes tiny frames to an actively
// draining peer, so a 300 ms ceiling never trips a healthy write.
fn ensure_fast_write_timeout_for_tests() {
    WRITE_TIMEOUT_ENV.get_or_init(|| unsafe {
        std::env::set_var("AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS", "300");
    });
}

// Spawns `serve_connection` without unwrapping its result, so a test can assert
// on the error-return paths (write timeout, invalid frame bytes).
fn spawn_relay_connection_capturing(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> (UnixStream, thread::JoinHandle<Result<(), std::io::Error>>) {
    let (mut server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let paths = bundle_paths.clone();
    let join_handle = thread::spawn(move || serve_connection(&mut server_stream, &root, &paths));
    (client_stream, join_handle)
}

// Clamps a socket's receive buffer to the kernel minimum so a non-reading peer
// fills its buffer after only a handful of small frames.
fn minimize_receive_buffer(stream: &UnixStream) {
    let size: libc::c_int = 1;
    let result = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            std::ptr::addr_of!(size).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    assert_eq!(result, 0, "failed to shrink socket receive buffer");
}

// Joins a captured `serve_connection` thread, failing the test if the worker
// stays pinned past the deadline instead of returning.
fn join_within(
    handle: thread::JoinHandle<Result<(), std::io::Error>>,
    timeout: Duration,
) -> Result<(), std::io::Error> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if handle.is_finished() {
            return handle.join().expect("join relay thread");
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("serve_connection did not return; connection-pool worker is still pinned");
}

fn agent_hello_frame(bundle_name: &str) -> Value {
    json!({
        "frame": "hello",
        "schema_version": "1",
        "bundle_name": bundle_name,
        "session_id": "alpha",
    })
}

#[test]
fn stalled_client_write_timeout_releases_connection_worker() {
    ensure_fast_write_timeout_for_tests();
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_write_timeout";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client_stream, join_handle) =
        spawn_relay_connection_capturing(&configuration_root, &bundle_paths);
    minimize_receive_buffer(&client_stream);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(&mut client_stream, agent_hello_frame(bundle_name));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    // Stop draining responses, then flood the relay with requests. The relay
    // writes one response per request into the shrunk client buffer; once it
    // is full the relay's write blocks, and the write timeout must tear the
    // connection down rather than pinning the worker indefinitely.
    drop(reader);
    let flood = thread::spawn(move || {
        for _ in 0..512 {
            let encoded = serde_json::to_string(&json!({
                "frame": "request",
                "request": {"operation": "list", "sender_session": "alpha"}
            }))
            .expect("encode request");
            if client_stream
                .write_all(format!("{encoded}\n").as_bytes())
                .is_err()
            {
                break;
            }
        }
        client_stream
    });

    let outcome = join_within(join_handle, Duration::from_secs(5));
    let error = outcome.expect_err("stalled-client write should fail");
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::BrokenPipe
        ),
        "unexpected error kind: {error:?}"
    );
    drop(flood.join().expect("join client flood"));

    // The connection must have been released from the registry: a fresh hello
    // with the same identity registers instead of conflicting.
    let (mut reconnect_client, reconnect_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let reconnect_read = reconnect_client
        .try_clone()
        .expect("clone reconnect stream");
    let mut reconnect_reader = BufReader::new(reconnect_read);
    send_json(&mut reconnect_client, agent_hello_frame(bundle_name));
    let reconnect_ack = read_json(&mut reconnect_reader);
    assert_eq!(reconnect_ack["frame"], "hello_ack");
    shutdown_stream(&reconnect_client, "shutdown reconnect client");
    reconnect_handle
        .join()
        .expect("join reconnect relay thread");
}

#[test]
fn connection_loop_error_releases_hello_claim() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_error_release";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client_stream, join_handle) =
        spawn_relay_connection_capturing(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(&mut client_stream, agent_hello_frame(bundle_name));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    // Invalid UTF-8 makes the relay's line read fail with a non-EOF error,
    // exercising the connection loop's error-return path.
    client_stream
        .write_all(&[0xff, 0xff, b'\n'])
        .expect("write invalid bytes");
    client_stream.flush().expect("flush invalid bytes");

    let outcome = join_within(join_handle, Duration::from_secs(5));
    assert!(outcome.is_err(), "expected connection loop to error");
    drop(reader);
    drop(client_stream);

    // The errored connection must release its registry entry so the same
    // identity can reconnect without an identity-claim conflict.
    let (mut reconnect_client, reconnect_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let reconnect_read = reconnect_client
        .try_clone()
        .expect("clone reconnect stream");
    let mut reconnect_reader = BufReader::new(reconnect_read);
    send_json(&mut reconnect_client, agent_hello_frame(bundle_name));
    let reconnect_ack = read_json(&mut reconnect_reader);
    assert_eq!(reconnect_ack["frame"], "hello_ack");
    shutdown_stream(&reconnect_client, "shutdown reconnect client");
    reconnect_handle
        .join()
        .expect("join reconnect relay thread");
}
