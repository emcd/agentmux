use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{io::AsRawFd, net::UnixStream},
    path::{Path, PathBuf},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use agentmux::{
    relay::{BundleCatalog, serve_connection},
    runtime::paths::BundleRuntimePaths,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::runtime::Builder as TokioRuntimeBuilder;

// Mirrors the production pre-hello idle timeout. Tests connect through the
// async `serve_connection`, which now drives reads on a tokio runtime; pre-
// hello idle gating is enforced inline by the connection task rather than on
// the OS stream.
const TEST_PRE_HELLO_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

#[path = "relay_stream/identity.rs"]
mod identity;
#[path = "relay_stream/list.rs"]
mod list;
#[path = "relay_stream/look.rs"]
mod look;
#[path = "relay_stream/permissions.rs"]
mod permissions;
#[path = "relay_stream/robustness.rs"]
mod robustness;
#[path = "relay_stream/routing.rs"]
mod routing;

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

// Derives a short, unique `@GLOBAL` operator id from a (per-test unique) bundle
// name. The stream registry is process-wide and keys relay-wide principals by
// `principal_id` alone, so concurrent tests sharing one global id would collide;
// hashing keeps the id distinct per test while staying within the session-id
// length limit (the raw bundle name can exceed it).
fn global_user_id(bundle_name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bundle_name.hash(&mut hasher);
    format!("g{:016x}@GLOBAL", hasher.finish())
}

// Writes a TUI configuration declaring one `@GLOBAL` operator whose id is unique
// per test (see `global_user_id`).
fn write_tui_configuration(configuration_root: &Path, policy: &str, bundle_name: &str) {
    let global_id = global_user_id(bundle_name);
    std::fs::write(
        configuration_root.join("users.toml"),
        format!(
            r#"
default-bundle = "{bundle_name}"
default-session = "{global_id}"

[[sessions]]
id = "{global_id}"
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

// Overwrites the policy artifact so the `default` preset's `send` control uses
// the given scope, leaving the other controls at workable defaults. Cross-bundle
// send requires `all:all`; `all:home` permits only same-bundle delivery.
fn write_policies_with_send(configuration_root: &Path, send: &str) {
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
list = "all:home"
look = "self"
send = "{send}"
"#
        ),
    )
    .expect("write policies configuration");
}

// Overwrites the policy artifact so the `default` preset's `raww` control uses
// the given scope. A relay-wide (`@GLOBAL`) principal reaching a bundle is
// cross-namespace, so `all:all` is required there; `all:home` permits only
// same-bundle raww.
fn write_policies_with_raww(configuration_root: &Path, raww: &str) {
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
list = "all:home"
look = "self"
raww = "{raww}"
send = "all:home"
"#
        ),
    )
    .expect("write policies configuration");
}

// Overwrites the policy artifact so the `default` preset's `list` control uses
// the given scope. Cross-bundle list requires `all:all`; `all:home` permits only
// same-bundle enumeration.
fn write_policies_with_list(configuration_root: &Path, list: &str) {
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
list = "{list}"
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
    for (index, (permission_request_id, option_id)) in entries.iter().enumerate() {
        let sequence = (index as u64) + 1;
        let requested_details = json!({
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
        });
        agentmux::relay::install_pending_permission_request_for_testing(
            runtime_directory,
            permission_request_id,
            format!("msg-{sequence}").as_str(),
            "alpha",
            "execute",
            requested_details,
        )
        .expect("install pending permission request for testing");
    }
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
    spawn_relay_connection_with_enforcement(configuration_root, bundle_paths, false)
}

fn spawn_relay_connection_with_enforcement(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    require_session_credentials: bool,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let state_root = bundle_paths.state_root.clone();
    let catalog = single_bundle_catalog(bundle_paths);
    let join_handle = thread::spawn(move || {
        run_serve_connection_with_enforcement(
            server_stream,
            root,
            state_root,
            catalog,
            require_session_credentials,
        )
        .expect("serve connection")
    });
    (client_stream, join_handle)
}

fn single_bundle_catalog(bundle_paths: &BundleRuntimePaths) -> BundleCatalog {
    BundleCatalog::from_paths([bundle_paths.clone()])
}

fn multi_bundle_catalog(bundle_paths: &[BundleRuntimePaths]) -> BundleCatalog {
    BundleCatalog::from_paths(bundle_paths.iter().cloned())
}

// Writes an additional bundle whose sole session `agent` is a coder-less UI
// target. Cross-bundle delivery to a UI session is observable on a registered
// relay stream (unlike a tmux member, which needs a live pane), so this backs
// the cross-namespace fan-out delivery assertions.
fn write_ui_bundle(configuration_root: &Path, bundle_name: &str) {
    let bundles_directory = configuration_root.join("bundles");
    std::fs::write(
        bundles_directory.join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "agent"
name = "Agent"
directory = "/tmp"

[sessions.ui]
"#,
    )
    .expect("write ui bundle configuration");
}

// Spawns a connection served against an explicit (typically multi-bundle)
// catalog, so a Send can fan out to peer bundles the connection is not bound to.
fn spawn_relay_connection_with_catalog(
    configuration_root: &Path,
    state_root: &Path,
    catalog: BundleCatalog,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let state = state_root.to_path_buf();
    let join_handle = thread::spawn(move || {
        run_serve_connection_with_enforcement(server_stream, root, state, catalog, false)
            .expect("serve connection")
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
    state_root: PathBuf,
    bundle_catalog: BundleCatalog,
) -> Result<(), std::io::Error> {
    run_serve_connection_with_enforcement(
        server_stream,
        configuration_root,
        state_root,
        bundle_catalog,
        false,
    )
}

fn run_serve_connection_with_enforcement(
    server_stream: UnixStream,
    configuration_root: PathBuf,
    state_root: PathBuf,
    bundle_catalog: BundleCatalog,
    require_session_credentials: bool,
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
            &state_root,
            &bundle_catalog,
            require_session_credentials,
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
            "request": {"operation": "list", "requester_session": "alpha"}
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
            "principal_id": format!("alpha@{bundle_name}"),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");
    assert_eq!(hello_ack["principal_id"], format!("alpha@{bundle_name}"));

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {"operation": "list", "requester_session": "alpha"}
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
        "principal_id": format!("alpha@{bundle_name}"),
        "identity_token": "socket-trust",
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
        second_response["response"]["error"]["details"]["principal_id"],
        format!("alpha@{bundle_name}")
    );
    assert_eq!(
        second_response["response"]["error"]["details"]["reason"],
        "existing identity owner is still live"
    );

    send_json(
        &mut first_client,
        json!({
            "frame": "request",
            "request": {"operation": "list", "requester_session": "alpha"}
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
            "principal_id": format!("alpha@{bundle_name}"),
            "identity_token": "socket-trust",
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
            "principal_id": format!("alpha@{bundle_name}"),
            "identity_token": "socket-trust",
        }),
    );
    let second_ack = read_json(&mut second_reader);
    assert_eq!(second_ack["frame"], "hello_ack");

    shutdown_stream(&second_client, "shutdown second client");
    second_handle.join().expect("join second relay thread");
}
