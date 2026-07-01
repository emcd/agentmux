use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use agentmux::{
    relay::{
        BundleCatalog, ConnectionDrainCoordinator, RelayRequest, RelayResponse, SendOutcome,
        handle_request, serve_connection,
    },
    runtime::paths::{BundleRuntimePaths, principal_store_path},
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
list = "home"
look = "self"
send = "home"

# A relay-wide operator's home namespace is GLOBAL, so reaching into a bundle is
# cross-namespace and requires all.
[[policies]]
id = "operator"

[policies.controls]
find = "self"
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
default-bundle = "example"
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
    configuration_root
}

fn spawn_relay_stream(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let state_root = bundle_paths.state_root.clone();
    let catalog = BundleCatalog::from_paths([bundle_paths.clone()]);
    let handle = thread::spawn(move || {
        run_serve_connection(server_stream, root, state_root, catalog).expect("serve connection");
    });
    (client_stream, handle)
}

// Bridges `serve_connection` (now async) into a synchronous test thread by
// owning a dedicated current-thread tokio runtime per connection.
fn run_serve_connection(
    server_stream: UnixStream,
    configuration_root: PathBuf,
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
            agentmux::relay::PeerConnectionManager::from_configuration(&state_root, &[]),
        );
        let serve_context = agentmux::relay::ConnectionServeContext::new(
            configuration_root,
            state_root,
            bundle_catalog,
            peer_connection_manager,
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
        if value["frame"] != "event" || value["event"]["target_session"] != target_session {
            continue;
        }
        let terminal = value["event"]["event_type"] == "delivery_outcome"
            && value["event"]["payload"]["phase"] == "delivered"
            && terminal_message_id
                .is_none_or(|id| value["event"]["payload"]["message_id"].as_str() == Some(id));
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

#[test]
fn relay_send_routes_to_connected_ui_stream_with_event_frames() {
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
        hello_payload(bundle_name.as_str(), &global_user_id(&bundle_name)),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-1".to_string()),
            requester_session: "alpha".to_string(),
            message: "hello ui".to_string(),
            targets: vec![global_user_id(&bundle_name)],
            broadcast: false,
            quiet_window_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("send response");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Events to a relay-wide (`@GLOBAL`) UI session are addressed to its full
    // principal id; sender attribution rides in the payload's `sender_session`.
    let events = collect_events_for_target(
        &ui_client,
        &mut reader,
        &global_user_id(&bundle_name),
        None,
        Duration::from_secs(3),
    );
    let incoming_event = events
        .iter()
        .find(|value| value["event"]["event_type"] == "incoming_message")
        .expect("incoming event");
    assert_eq!(
        incoming_event["event"]["target_session"],
        global_user_id(&bundle_name)
    );
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
fn relay_send_waits_for_ui_reconnect_before_delivery() {
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
        hello_payload(bundle_name.as_str(), &global_user_id(&bundle_name)),
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
            hello_payload(
                reconnect_bundle.as_str(),
                &global_user_id(&reconnect_bundle),
            ),
        );
        let ack = read_json(&mut reconnect_reader);
        let events = collect_events_for_target(
            &reconnect_client,
            &mut reconnect_reader,
            &global_user_id(&reconnect_bundle),
            None,
            Duration::from_secs(3),
        );
        reconnect_client
            .shutdown(std::net::Shutdown::Both)
            .expect("shutdown reconnect stream");
        (ack, events)
    });

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-2".to_string()),
            requester_session: "alpha".to_string(),
            message: "wait for reconnect".to_string(),
            targets: vec![global_user_id(&bundle_name)],
            broadcast: false,
            quiet_window_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Async dispatch returns immediately; the reconnect thread connects at
    // +150ms and still receives the terminal delivered/success event, which
    // proves the background worker held delivery until the UI reconnected.
    let (ack, events) = reconnect_thread.join().expect("join reconnect thread");
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
fn relay_async_send_emits_terminal_delivery_outcome_to_sender_ui_stream() {
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
        hello_payload(bundle_name.as_str(), &global_user_id(&bundle_name)),
    );
    let sender_ack = read_json(&mut sender_reader);
    assert_eq!(sender_ack["frame"], "hello_ack");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-async-sender".to_string()),
            requester_session: global_user_id(&bundle_name),
            message: "verify sender completion stream".to_string(),
            targets: vec![format!("alpha@{bundle_name}")],
            broadcast: false,
            quiet_window_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("send response");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);
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

// Writes the standard configuration plus a configured (non-`@GLOBAL`) `display`
// UI member, so a send can target a configured UI session whose UI stream may
// register after the first delivery.
fn write_bundle_configuration_with_ui_member(temporary: &TempDir, bundle_name: &str) -> PathBuf {
    let configuration_root = write_bundle_configuration(temporary, bundle_name);
    let bundles_directory = configuration_root.join("bundles");
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
    configuration_root
}

// Regression: a configured UI target whose first send precedes its UI stream
// registration must recover and deliver via `UiTransport` once the stream
// connects. The per-target worker resolves UI routing per delivery (not latched
// for its lifetime), so the second send routes to UI even though the first bound
// the worker before any UI stream existed.
#[test]
fn relay_configured_ui_target_recovers_after_late_stream_registration() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let display_target = format!("display@{bundle_name}");

    // Send #1 to the configured UI target BEFORE any UI stream registers. The
    // per-target worker is created and routes this delivery on the non-UI path
    // (the registry has no UI entry yet). Under a latched resolution this would
    // bind the worker to non-UI for its lifetime.
    let first = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-ui-pre".to_string()),
            requester_session: "alpha".to_string(),
            message: "before registration".to_string(),
            targets: vec![display_target.clone()],
            broadcast: false,
            quiet_window_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("first send response");
    let RelayResponse::Send { results, .. } = first else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Let the worker process send #1 so the routing decision is exercised once
    // while no UI stream exists.
    thread::sleep(Duration::from_millis(300));

    // The UI client now connects and registers a Ui stream for `display`.
    let (mut ui_client, ui_handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let read_stream = ui_client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut ui_client,
        hello_payload(bundle_name.as_str(), "display"),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    // Send #2 to the same target: the worker must re-evaluate routing, observe
    // the now-registered UI stream, and deliver via UiTransport.
    let second = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-ui-post".to_string()),
            requester_session: "alpha".to_string(),
            message: "after registration".to_string(),
            targets: vec![display_target.clone()],
            broadcast: false,
            quiet_window_ms: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("second send response");
    let RelayResponse::Send {
        results: second_results,
        ..
    } = second
    else {
        panic!("expected send response");
    };
    assert_eq!(second_results.len(), 1);
    assert_eq!(second_results[0].outcome, SendOutcome::Queued);

    // Collect until the SECOND message's terminal outcome. The first message was
    // held before any UI stream existed; it flushes (with its own `delivered`
    // outcome) the moment the stream registers, ahead of the second send's events.
    // Keying the terminal on the second message id keeps collection going past the
    // held message so the "after registration" `incoming_message` is observed.
    let events = collect_events_for_target(
        &ui_client,
        &mut reader,
        display_target.as_str(),
        Some(second_results[0].message_id.as_str()),
        Duration::from_secs(3),
    );
    events
        .iter()
        .find(|value| {
            value["event"]["event_type"] == "incoming_message"
                && value["event"]["payload"]["body"] == "after registration"
        })
        .expect("'after registration' incoming_message after late UI stream registration");
    let delivered = events
        .iter()
        .find(|value| {
            value["event"]["event_type"] == "delivery_outcome"
                && value["event"]["payload"]["phase"] == "delivered"
                && value["event"]["payload"]["message_id"] == second_results[0].message_id.as_str()
        })
        .expect("delivered outcome for second send after late registration");
    assert_eq!(delivered["event"]["payload"]["outcome"], "success");
    assert_eq!(
        delivered["event"]["payload"]["message_id"],
        second_results[0].message_id
    );

    ui_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown ui stream");
    ui_handle.join().expect("join relay stream");
}

// ---------------------------------------------------------------------------
// Choose-authorized choices list and submitter gating
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
list = "home"
look = "self"
send = "home"
choose = "none"

[[policies]]
id = "operator"

[policies.controls]
find = "self"
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
    assert_eq!(hello_ack["principal_id"], format!("alpha@{bundle_name}"));

    client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown stream");
    handle.join().expect("join relay stream");
}

#[test]
fn relay_choices_list_succeeds_for_grant_authorized_principal() {
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
            "request": {"operation": "choices_list"},
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], request_id);
    assert_eq!(response["response"]["kind"], "choices_list");
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
fn relay_choices_pick_rejects_submitter_without_grant() {
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
                "operation": "choices_pick",
                "choice_request_id": "perm-1",
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

// === Cross-relay peer forwarding (tasks 6.4/6.5) ===
//
// These exercise the outbound Send fan-out end to end: a bundle member on the
// origin relay addresses a cross-relay bang-path target, the origin dials a stub
// peer relay (PSK Hello), forwards the Send, and folds the peer's response (or a
// transport failure) into the merged Send result.

// Bundle whose member `alpha` holds `send = all`, so it may address a cross-relay
// (always all-tier) target. Otherwise a minimal single-member tmux bundle.
fn write_cross_relay_bundle_configuration(temporary: &TempDir, bundle_name: &str) -> PathBuf {
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
default = "peer_sender"

[[policies]]
id = "peer_sender"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "all"
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
    configuration_root
}

// Writes the outbound PSK for `alias` at `<state-root>/peers/<alias>.psk`, where
// the peer connection manager reads it on first forward.
fn write_peer_credential(state_root: &Path, alias: &str, psk: &str) {
    let peers_dir = state_root.join("peers");
    std::fs::create_dir_all(&peers_dir).expect("create peers dir");
    std::fs::write(peers_dir.join(format!("{alias}.psk")), psk).expect("write peer psk");
}

// Serves one origin relay connection whose peer connection manager is configured
// with a single peer relay: `alias` is the bang-path `!<alias>` selector,
// `connect_as` is the identity this relay presents to the peer (`<connect_as>@RELAY`),
// and `peer_socket` is the peer's Unix socket. So a cross-relay Send is really
// dialed and forwarded rather than reported unavailable.
fn spawn_relay_stream_with_peer(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    alias: &str,
    connect_as: &str,
    peer_socket: &Path,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let state_root = bundle_paths.state_root.clone();
    let catalog = BundleCatalog::from_paths([bundle_paths.clone()]);
    let peers = vec![agentmux::relay::PeerConfiguration {
        alias: alias.to_string(),
        address: peer_socket.to_string_lossy().into_owned(),
        connect_as: connect_as.to_string(),
    }];
    let handle = thread::spawn(move || {
        run_serve_connection_with_peers(server_stream, root, state_root, catalog, peers)
            .expect("serve connection");
    });
    (client_stream, handle)
}

fn run_serve_connection_with_peers(
    server_stream: UnixStream,
    configuration_root: PathBuf,
    state_root: PathBuf,
    bundle_catalog: BundleCatalog,
    peers: Vec<agentmux::relay::PeerConfiguration>,
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
        let peer_connection_manager = std::sync::Arc::new(
            agentmux::relay::PeerConnectionManager::from_configuration(&state_root, &peers),
        );
        let serve_context = agentmux::relay::ConnectionServeContext::new(
            configuration_root,
            state_root,
            bundle_catalog,
            peer_connection_manager,
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

// A one-shot stub peer relay: completes the PSK Hello handshake (echoing the
// dialer's schema_version + principal_id) and answers the single forwarded
// request with `response`, echoing the wire request id for correlation. Returns
// the request frame it observed.
fn spawn_answering_peer(socket_path: &Path, response: Value) -> mpsc::Receiver<Value> {
    let listener = UnixListener::bind(socket_path).expect("bind stub peer socket");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone stub stream"));
        let mut stream = stream;
        let mut hello_line = String::new();
        if reader.read_line(&mut hello_line).is_err() {
            return;
        }
        let Ok(hello) = serde_json::from_str::<Value>(hello_line.trim_end()) else {
            return;
        };
        let ack = json!({
            "frame": "hello_ack",
            "schema_version": hello.get("schema_version").cloned().unwrap_or(json!("")),
            "principal_id": hello.get("principal_id").cloned().unwrap_or(json!("")),
        });
        let _ = writeln!(stream, "{ack}");
        let _ = stream.flush();
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_ok()
            && let Ok(request) = serde_json::from_str::<Value>(request_line.trim_end())
        {
            let response_frame = json!({
                "frame": "response",
                "request_id": request.get("request_id").cloned().unwrap_or(json!(null)),
                "response": response,
            });
            let _ = writeln!(stream, "{response_frame}");
            let _ = stream.flush();
            let _ = sender.send(request);
            thread::sleep(Duration::from_millis(50));
        }
    });
    receiver
}

// Hellos as `alpha`, sends one cross-relay Send over the stream, and returns the
// decoded `send` response's `results` array plus the request frame the stub peer
// observed. Shared by the delivered/ingress-denied cases.
fn forward_cross_relay_send(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    peer_socket: &Path,
    observed: mpsc::Receiver<Value>,
) -> (Vec<Value>, Value) {
    let (mut client, handle) = spawn_relay_stream_with_peer(
        configuration_root,
        bundle_paths,
        "peer",
        "origin-relay",
        peer_socket,
    );
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(&mut client, hello_payload(bundle_name, "alpha"));
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {
                "operation": "send",
                "requester_session": "alpha",
                "message": "cross-relay hello",
                "targets": ["bravo@other!peer"],
                "broadcast": false,
            },
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array")
        .clone();
    let forwarded = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("stub peer observed the forwarded send");

    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
    (results, forwarded)
}

#[test]
fn cross_relay_send_propagates_peer_delivery_outcome() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_cross_relay_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");

    let peer_socket = temporary.path().join("peer.sock");
    // The peer answers with a queued delivery result for its local target; the
    // origin re-labels it to the bang-path the requester addressed.
    let peer_response = json!({
        "kind": "send",
        "schema_version": "test",
        "requester_session": "origin-relay@RELAY",
        "results": [{
            "target_session": "bravo@other",
            "message_id": "peer-m1",
            "outcome": "queued",
        }],
    });
    let observed = spawn_answering_peer(&peer_socket, peer_response);

    let (results, forwarded) = forward_cross_relay_send(
        &configuration_root,
        &bundle_paths,
        bundle_name.as_str(),
        &peer_socket,
        observed,
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["target_session"], "bravo@other!peer");
    assert_eq!(results[0]["outcome"], "queued");

    // The peer received a Send presenting this relay's `<relay-id>@RELAY` identity
    // and its plain local target (not the bang-path).
    assert_eq!(forwarded["request"]["operation"], "send");
    assert_eq!(
        forwarded["request"]["requester_session"],
        "origin-relay@RELAY"
    );
    assert_eq!(forwarded["request"]["targets"][0], "bravo@other");
}

#[test]
fn cross_relay_send_reports_ingress_denied_as_failed() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_cross_relay_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");

    let peer_socket = temporary.path().join("peer.sock");
    // The peer rejects the forwarded target at its ingress filter; the origin
    // folds that error into a `failed` result carrying the peer's reason code.
    let peer_response = json!({
        "kind": "error",
        "error": {
            "code": "authorization_forbidden",
            "message": "ingress scope does not cover this target",
        },
    });
    let observed = spawn_answering_peer(&peer_socket, peer_response);

    let (results, _forwarded) = forward_cross_relay_send(
        &configuration_root,
        &bundle_paths,
        bundle_name.as_str(),
        &peer_socket,
        observed,
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["target_session"], "bravo@other!peer");
    assert_eq!(results[0]["outcome"], "failed");
    assert_eq!(results[0]["reason_code"], "authorization_forbidden");
}

#[test]
fn cross_relay_send_reports_peer_unavailable_when_unreachable() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_cross_relay_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");

    // No listener is bound at this path: the peer is unreachable. The origin still
    // serves (an unreachable peer never blocks boot — the connection is lazy), and
    // the first delivery to it yields the distinct `peer_unavailable` outcome.
    let peer_socket = temporary.path().join("nonexistent-peer.sock");
    let (mut client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "peer",
        "origin-relay",
        &peer_socket,
    );
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {
                "operation": "send",
                "requester_session": "alpha",
                "message": "cross-relay hello",
                "targets": ["bravo@other!peer"],
                "broadcast": false,
            },
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["target_session"], "bravo@other!peer");
    assert_eq!(results[0]["outcome"], "peer_unavailable");
    assert_eq!(results[0]["reason_code"], "runtime_peer_unavailable");

    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
}

#[test]
fn cross_relay_send_requires_all_tier() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    // The default policy grants alpha `send = home`, so a cross-relay (always
    // all-tier) target is denied at authorization before any peer is dialed.
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");

    // A peer is configured, so the request is not reported as unavailable; the
    // socket is never dialed because authorization fails first (no listener bound).
    let peer_socket = temporary.path().join("unused-peer.sock");
    let (mut client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "peer",
        "origin-relay",
        &peer_socket,
    );
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {
                "operation": "send",
                "requester_session": "alpha",
                "message": "cross-relay hello",
                "targets": ["bravo@other!peer"],
                "broadcast": false,
            },
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );

    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
}

// === Cross-relay ingress filter (tasks 5.x, 6.3) ===
//
// These exercise the receiving side: a client authenticates as a peer relay
// principal (`<id>@RELAY`) whose store record carries a registered ingress scope,
// then forwards a Send to a local target. The receiving relay gates each target
// against the peer's scope (deny-by-default), preserving existence-before-
// authorization ordering.

// SHA-256 (lowercase hex) of the fixed ingress PSK below, embedded in the
// hand-written principal store so a Hello with the raw token authenticates.
const INGRESS_PEER_TOKEN: &str = "ingress-peer-secret";
const INGRESS_PEER_CREDENTIAL_HASH: &str =
    "1c9c2d8823d0f52409743bb29168008f69157c166de636fcca23631e26f8daa7";

// A per-test-unique peer relay principal id. The process-wide stream registry is
// keyed by `principal_id`, so concurrent tests must not share one or they collide
// with an identity-claim conflict. The credential hash is fixed (tied to the one
// token), independent of the id.
fn unique_relay_principal_id() -> String {
    format!("origin-{}@RELAY", Uuid::new_v4().simple())
}

// Hand-writes a principal store registering `principal_id` as a peer relay with
// the fixed ingress credential and the given `scope` (`None` for a peer
// registered without a scope, which the ingress gate treats as fail-closed).
fn write_ingress_peer_store(state_root: &Path, principal_id: &str, scope: Option<&str>) {
    let store_path = principal_store_path(state_root);
    std::fs::create_dir_all(store_path.parent().expect("store parent"))
        .expect("create identity directory");
    let scope_field = match scope {
        Some(value) => format!(",\n      \"scope\": \"{value}\""),
        None => String::new(),
    };
    let body = format!(
        "{{\n  \"format_version\": 1,\n  \"principals\": [\n    {{\n      \"principal_id\": \"{principal_id}\",\n      \"principal_type\": \"relay\",\n      \"credential_hash\": \"{INGRESS_PEER_CREDENTIAL_HASH}\"{scope_field}\n    }}\n  ]\n}}"
    );
    std::fs::write(&store_path, body).expect("write principal store");
}

// Hellos as `principal_id` (a peer relay) with its registered PSK, submits one
// request, and returns the decoded response frame.
fn ingress_request_response(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    request: Value,
) -> Value {
    let (mut client, handle) = spawn_relay_stream(configuration_root, bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": INGRESS_PEER_TOKEN,
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": request,
        }),
    );
    let response = read_json(&mut reader);
    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
    response
}

// Convenience wrapper: forwards an ingress Send to a single `target`.
fn ingress_send_response(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    target: &str,
) -> Value {
    ingress_request_response(
        configuration_root,
        bundle_paths,
        principal_id,
        json!({
            "operation": "send",
            "requester_session": principal_id,
            "message": "ingress hello",
            "targets": [target],
            "broadcast": false,
        }),
    )
}

#[test]
fn cross_relay_ingress_accepts_in_scope_target() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // A bundle-wide scope covers every session in the target bundle.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );

    let response = ingress_send_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        format!("display@{bundle_name}").as_str(),
    );
    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["outcome"], "queued");
    assert_eq!(
        results[0]["target_session"],
        format!("display@{bundle_name}")
    );
}

#[test]
fn cross_relay_ingress_denies_out_of_scope_target() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // The scope names a different bundle, so the target is out of scope.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("some-other-bundle"),
    );

    let response = ingress_send_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        format!("display@{bundle_name}").as_str(),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "ingress"
    );
}

#[test]
fn cross_relay_ingress_denies_absent_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // A peer registered without a scope covers nothing (deny-by-default).
    write_ingress_peer_store(&bundle_paths.state_root, relay_principal_id.as_str(), None);

    let response = ingress_send_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        format!("display@{bundle_name}").as_str(),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
}

#[test]
fn cross_relay_ingress_unknown_target_sorts_before_authorization() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // Out-of-scope peer targeting a non-member: existence is validated by the
    // spine's prepare stage before the ingress gate, so an unknown target sorts
    // as `validation_unknown_target` rather than `authorization_forbidden`.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("some-other-bundle"),
    );

    let response = ingress_send_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        format!("ghost@{bundle_name}").as_str(),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_target"
    );
}

#[test]
fn cross_relay_ingress_rejects_bang_path_send_before_forwarding() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // In-scope for the local bundle, but the target carries a bang-path onward
    // relay id. A peer must not chain through this relay to a third relay, so the
    // request is rejected before any forwarding — the receiving relay here has no
    // peer manager configured, proving the safety does not depend on a third peer.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );

    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({
            "operation": "send",
            "requester_session": relay_principal_id,
            "message": "chain attempt",
            "targets": [format!("display@{bundle_name}!third")],
            "broadcast": false,
        }),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "ingress"
    );
}

#[test]
fn cross_relay_ingress_rejects_bang_path_raww_before_forwarding() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );

    // Raww's cross-relay branch runs before the local spine; an ingress bang-path
    // target must be rejected there, not forwarded onward under this relay's
    // identity.
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({
            "operation": "raww",
            "requester_session": relay_principal_id,
            "target_session": format!("display@{bundle_name}!third"),
            "text": "chain attempt",
            "no_enter": false,
        }),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "ingress"
    );
}
