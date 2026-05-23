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
use tokio::runtime::Builder as TokioRuntimeBuilder;

// Mirrors the production pre-hello idle timeout. Tests connect through the
// async `serve_connection`, which now drives reads on a tokio runtime; pre-
// hello idle gating is enforced inline by the connection task rather than on
// the OS stream.
const TEST_PRE_HELLO_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

#[path = "relay_stream/permissions.rs"]
mod permissions;
#[path = "relay_stream/robustness.rs"]
mod robustness;

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
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let paths = bundle_paths.clone();
    let join_handle = thread::spawn(move || {
        run_serve_connection(server_stream, root, paths).expect("serve connection")
    });
    (client_stream, join_handle)
}

// Bridges a synchronous unit test to the async `serve_connection`. Each
// connection gets its own two-worker tokio runtime so the writer task can
// observe its write timeout while the connection task is in a tight
// read/dispatch loop (a single-worker runtime starves the writer until the
// reader yields). Matches the production runtime flavor.
fn run_serve_connection(
    server_stream: UnixStream,
    configuration_root: PathBuf,
    bundle_paths: BundleRuntimePaths,
) -> Result<(), std::io::Error> {
    server_stream.set_nonblocking(true)?;
    let runtime = TokioRuntimeBuilder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build multi-thread runtime");
    runtime.block_on(async move {
        let stream = tokio::net::UnixStream::from_std(server_stream)?;
        serve_connection(
            stream,
            &configuration_root,
            &bundle_paths,
            TEST_PRE_HELLO_IDLE_TIMEOUT,
        )
        .await
    })
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

// Reads the next frame, tolerating a stray `hello_ack` in the rare case the
// connection observes one (kept defensive even after the probe-based liveness
// check was removed; the production stream client still tolerates it).
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
    // The production client tolerates a stray `hello_ack` and the legacy
    // helper still skips one defensively, even though the async migration
    // removed the conflict probe.
    let first_response = read_json_skipping_hello_ack(&mut first_reader);
    assert_eq!(first_response["frame"], "response");
    assert_eq!(first_response["response"]["kind"], "list");

    shutdown_stream(&first_client, "shutdown first client");
    shutdown_stream(&second_client, "shutdown second client");
    first_handle.join().expect("join first relay thread");
    second_handle.join().expect("join second relay thread");
}

// The synchronous identity-conflict liveness probe was removed during the
// async migration: stale-but-attached owners now surface conflicts until a
// real relay-to-client write fails and the writer task closes the registry's
// sender. The standard client retry on `runtime_identity_claim_conflict`
// covers the reconnect path. (Previously a Linux-only test asserted probe-
// driven eviction here; see todos/relay/48.)

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
