//! Integration tests for `serve_connection` end-to-end: stream-based UI
//! delivery, configured-session registration, choose authorization, and the
//! cross-relay (bang-path) send/ingress surface.
//!
//! The cluster files partition the 22 tests by concern:
//! - [`ui_delivery`]: connected UI stream event delivery (routing, reconnect
//!   hold, late-stream registration, async-sender outcome).
//! - [`registration`]: configured bundle member hello/registration.
//! - [`choices`]: choose-authorized list and pick rejection.
//! - [`cross_relay`]: outbound bang-path Send forwarding and ingress filter
//!   (peer-scope gate).
//! - [`on_behalf_of`]: cross-relay sender-attribution (`on_behalf_of`)
//!   stamping, surfacing, and spoof-gate behaviour.
//!
//! Shared helpers (every cluster shares the stream registry, the
//! per-bundle configuration writer, the per-test-unique `@GLOBAL` id, and
//! the JSON line codec) live in this hub. The cross-relay peer fixtures
//! (stub peers, forward drivers) live in [`peer_support`], and the ingress
//! peer store fixtures live in [`ingress_support`]; both are re-exported
//! here so clusters keep using them through this hub.
//! Cluster-specific helpers live with their cluster.

use agentmux::configuration::ConfigurationRoots;
use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use agentmux::{
    relay::{
        BundleCatalog, ConnectionDrainCoordinator, RelayRequest,
        RelayResponse, handle_request, serve_connection,
    },
    runtime::paths::BundleRuntimePaths,
};
use serde_json::{Value, json};
use tempfile::TempDir;

mod choices;
mod cross_relay;
mod discovery;
mod ingress_support;
mod on_behalf_of;
mod peer_support;
mod registration;
mod ui_delivery;

pub(crate) use ingress_support::{
    INGRESS_PEER_TOKEN, UI_DELIVERY_WORKER_BUDGET, ingress_request_response,
    ingress_send_response, unique_relay_principal_id,
    write_authenticated_session_store, write_ingress_peer_store,
};
pub(crate) use peer_support::{
    ForwardedCrossRelaySend, forward_cross_relay_raww,
    forward_cross_relay_send, forward_cross_relay_send_supplying_on_behalf_of,
    forward_cross_relay_send_with_hello, spawn_answering_peer,
    spawn_relay_stream_with_peer, spawn_revoking_peer, write_peer_credential,
};

fn dispatch_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(
        request,
        configuration_roots,
        bundle_name,
        runtime_directory,
    )
}

fn write_bundle_configuration(
    temporary: &TempDir,
    bundle_name: &str,
) -> ConfigurationRoots {
    let configuration_root = temporary.path().join("config");
    let bundles_directory = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles_directory)
        .expect("create bundles directory");
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
list = "home"
look = "self"
send = "home"

# A relay-wide operator's home namespace is GLOBAL, so reaching into a bundle is
# cross-namespace and requires all.
[[policies]]
id = "operator"

[policies.controls]
list = "all"
look = "all"
send = "all"
"#,
    )
    .expect("write policies configuration");
    let global_id = global_user_id(bundle_name);
    std::fs::write(
        configuration_root.join("users.toml"),
        format!(
            r#"
default-session = "{global_id}"

[[sessions]]
id = "{global_id}"
policy = "operator"

[sessions.ui]
"#
        ),
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
    ConfigurationRoots::single(configuration_root)
}

fn spawn_relay_stream(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) =
        UnixStream::pair().expect("unix stream pair");
    let root = configuration_roots.clone();
    let state_root = bundle_paths.state_root.clone();
    let catalog = BundleCatalog::from_paths([bundle_paths.clone()]);
    let handle = thread::spawn(move || {
        run_serve_connection(server_stream, root, state_root, catalog)
            .expect("serve connection");
    });
    (client_stream, handle)
}

// Bridges `serve_connection` (now async) into a synchronous test thread by
// owning a dedicated current-thread tokio runtime per connection.
fn run_serve_connection(
    server_stream: UnixStream,
    configuration_roots: ConfigurationRoots,
    state_root: PathBuf,
    bundle_catalog: BundleCatalog,
) -> Result<(), std::io::Error> {
    server_stream
        .set_nonblocking(true)
        .expect("non-blocking server stream");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime");
    runtime.block_on(async move {
        let stream = tokio::net::UnixStream::from_std(server_stream)?;
        // No peers configured for these tests: an empty manager never dials.
        let peer_connection_manager = std::sync::Arc::new(
            agentmux::relay::PeerConnectionManager::from_configuration(
                &state_root,
                &[],
            ),
        );
        let serve_context = agentmux::relay::ConnectionServeContext::new(
            configuration_roots,
            state_root,
            bundle_catalog,
            peer_connection_manager,
            Vec::new(),
            false,
            Duration::from_secs(2),
        );
        serve_connection(
            stream,
            &serve_context,
            ConnectionDrainCoordinator::new().register_worker(),
        )
        .await
    })
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

fn read_json_with_timeout(
    reader: &mut BufReader<UnixStream>,
) -> Option<Value> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(read) => {
            if read == 0 {
                return None;
            }
            Some(
                serde_json::from_str::<Value>(line.trim_end())
                    .expect("decode payload"),
            )
        },
        Err(source)
            if matches!(
                source.kind(),
                ErrorKind::TimedOut | ErrorKind::WouldBlock
            ) =>
        {
            None
        },
        Err(source) => panic!("read payload: {source}"),
    }
}

// Derives a short, unique `@GLOBAL` operator id from a (per-test unique) bundle
// name. Relay-wide principals are keyed in the process-wide stream registry by
// `principal_id` alone, so concurrent tests must not share one global id.
fn global_user_id(bundle_name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bundle_name.hash(&mut hasher);
    format!("g{:016x}@GLOBAL", hasher.finish())
}

// Collects stream events addressed to `target_session` until a terminal
// `delivered` outcome is seen or the deadline elapses. `terminal_message_id`
// selects which delivery ends collection: `Some(id)` stops only on the
// `delivered` outcome of that specific message, while `None` stops on the first
// `delivered` for any message. A specific id is required whenever an earlier
// delivery's outcome can reach the stream first — e.g. a message held before the
// UI stream existed is flushed (with its own `delivered` outcome) the moment a
// late stream registers, ahead of a later send's events. Passing `None` there
// would break collection on the held message and miss the later one. Relay-wide
// (`@GLOBAL`) UI connections receive events from every bundle on the relay, and
// the stream registry is process-wide, so a test must filter foreign events out
// by the recipient id in the canonical `target_session` (unique per test) rather
// than reading a fixed event count.
fn collect_events_for_target(
    stream: &UnixStream,
    reader: &mut BufReader<UnixStream>,
    target_session: &str,
    terminal_message_id: Option<&str>,
    deadline: Duration,
) -> Vec<Value> {
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set read timeout");
    let end = std::time::Instant::now() + deadline;
    let mut events = Vec::new();
    while std::time::Instant::now() < end {
        let Some(value) = read_json_with_timeout(reader) else {
            continue;
        };
        if value["frame"] != "event"
            || value["event"]["target_session"] != target_session
        {
            continue;
        }
        let terminal = value["event"]["event_type"] == "delivery_outcome"
            && value["event"]["payload"]["phase"] == "delivered"
            && terminal_message_id.is_none_or(|id| {
                value["event"]["payload"]["message_id"].as_str() == Some(id)
            });
        events.push(value);
        if terminal {
            break;
        }
    }
    let _ = stream.set_read_timeout(None);
    events
}

fn hello_payload(bundle_name: &str, session_id: &str) -> Value {
    let principal_id = if session_id.ends_with("@GLOBAL") {
        session_id.to_string()
    } else {
        format!("{session_id}@{bundle_name}")
    };
    json!({
        "frame": "hello",
        "schema_version": "1",
        "principal_id": principal_id,
        "identity_token": "socket-trust",
    })
}

// Writes the standard configuration plus a configured (non-`@GLOBAL`) `display`
// UI member, so a send can target a configured UI session whose UI stream may
// register after the first delivery.
fn write_bundle_configuration_with_ui_member(
    temporary: &TempDir,
    bundle_name: &str,
) -> ConfigurationRoots {
    let configuration_roots =
        write_bundle_configuration(temporary, bundle_name);
    let bundles_directory = configuration_roots.base_layer().join("bundles");
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
id = "display"
name = "Display"
directory = "/tmp"

[sessions.ui]
"#,
    )
    .expect("rewrite bundle configuration with ui member");
    configuration_roots
}

fn write_operator_bundle_configuration(
    temporary: &TempDir,
    bundle_name: &str,
) -> ConfigurationRoots {
    let configuration_root = temporary.path().join("config");
    let bundles_directory = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles_directory)
        .expect("create bundles directory");
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
list = "home"
look = "self"
send = "home"
choose = "none"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "home"
send = "home"
choose = "home"
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
    ConfigurationRoots::single(configuration_root)
}

// Bundle whose member `alpha` holds `send = all`, so it may address a cross-relay
// (always all-tier) target. Otherwise a minimal single-member tmux bundle.
fn write_cross_relay_bundle_configuration(
    temporary: &TempDir,
    bundle_name: &str,
) -> ConfigurationRoots {
    let configuration_root = temporary.path().join("config");
    let bundles_directory = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles_directory)
        .expect("create bundles directory");
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
default = "peer_sender"

[[policies]]
id = "peer_sender"

[policies.controls]
list = "home"
look = "self"
send = "all"
raww = "all"
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
"#,
    )
    .expect("write bundle configuration");
    ConfigurationRoots::single(configuration_root)
}
