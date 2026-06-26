use super::*;

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
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let state_root = bundle_paths.state_root.clone();
    let catalog = super::single_bundle_catalog(bundle_paths);
    let join_handle =
        thread::spawn(move || run_serve_connection(server_stream, root, state_root, catalog));
    (client_stream, join_handle)
}

// Clamps a socket's receive buffer to the kernel minimum so a non-reading peer
// fills its buffer after only a handful of small frames. Returns the actual
// clamped size reported by `getsockopt`; the kernel may round up significantly
// (Linux usually doubles for accounting overhead; macOS enforces a higher
// minimum than Linux, ~2 KiB). The caller uses this to size a flood that is
// guaranteed to overrun the buffer on any platform.
fn minimize_receive_buffer(stream: &UnixStream) -> usize {
    let requested: libc::c_int = 1;
    let result = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            std::ptr::addr_of!(requested).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    assert_eq!(result, 0, "failed to shrink socket receive buffer");

    let mut actual: libc::c_int = 0;
    let mut actual_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            std::ptr::addr_of_mut!(actual).cast(),
            std::ptr::addr_of_mut!(actual_len),
        )
    };
    assert_eq!(result, 0, "failed to read back SO_RCVBUF");
    assert!(actual >= 0, "kernel reported negative SO_RCVBUF: {actual}");
    actual as usize
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
    hello_frame_for("alpha", bundle_name)
}

fn hello_frame_for(session_id: &str, bundle_name: &str) -> Value {
    json!({
        "frame": "hello",
        "schema_version": "1",
        "principal_id": format!("{session_id}@{bundle_name}"),
        "identity_token": "socket-trust",
    })
}

// Overwrites the bundle file with a tmux sender `alpha` plus a coder-less UI
// member `panel`. Delivery to a UI member is pushed over its registered stream
// (no live tmux pane needed), which lets a sender flood events at an idle UI
// connection to drive the relay-to-client write timeout.
fn write_sender_and_ui_bundle(configuration_root: &Path, bundle_name: &str) {
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
id = "panel"
name = "Panel"
directory = "/tmp"

[sessions.ui]
"#,
    )
    .expect("write sender+ui bundle configuration");
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
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(&mut client_stream, agent_hello_frame(bundle_name));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    // Minimize the receive buffer only after the handshake completes. On
    // macOS, shrinking SO_RCVBUF before the first relay write causes the
    // hello_ack write to block immediately (Unix domain socket send/recv
    // buffer coupling differs from Linux), triggering the write timeout and
    // tearing the connection down before the client reads the ack.
    let rcvbuf_bytes = minimize_receive_buffer(&client_stream);

    // Stop draining responses, then flood the relay with requests. The relay
    // writes one response per request into the shrunk client buffer; once it
    // is full the relay's write blocks, and the write timeout must tear the
    // connection down rather than pinning the worker indefinitely.
    //
    // Flood size is derived from the actual receive-buffer capacity reported
    // by getsockopt above. The conservative lower bound of 32 bytes per
    // response, multiplied by a 4x safety factor, guarantees the flood
    // overruns the buffer on every platform (Linux ~2 KiB and macOS ~2-8 KiB
    // both fall well inside the resulting count). A 512-floor preserves the
    // historical Linux flood size in case getsockopt reports something
    // unusually small. See issues/relay/19.
    const ASSUMED_MIN_RESPONSE_BYTES: usize = 32;
    const FLOOD_SAFETY_MULTIPLIER: usize = 4;
    let flood_count =
        (rcvbuf_bytes / ASSUMED_MIN_RESPONSE_BYTES * FLOOD_SAFETY_MULTIPLIER).max(512);
    drop(reader);
    let flood = thread::spawn(move || {
        for _ in 0..flood_count {
            let encoded = serde_json::to_string(&json!({
                "frame": "request",
                "request": {"operation": "list", "requester_session": "alpha"}
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

    // Request dispatch runs on the blocking pool, so when the writer dies the
    // read loop may be parked awaiting a dispatched response. Teardown then
    // races the writer-exit arm (clean exit) against the next failed response
    // write (io error); both are valid. What must hold either way: the worker
    // is released promptly rather than pinned on the stalled write.
    let outcome = join_within(join_handle, Duration::from_secs(5));
    if let Err(error) = outcome {
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::BrokenPipe
            ),
            "unexpected error kind: {error:?}"
        );
    }
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
fn idle_ui_stream_write_timeout_tears_down_connection() {
    ensure_fast_write_timeout_for_tests();
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_idle_ui_timeout";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_sender_and_ui_bundle(&configuration_root, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // UI client: register, read the ack, shrink its receive buffer, then go
    // idle -- it never sends another frame and never reads again. This is the
    // TUI-regression shape: an idle SessionType::Ui stream the relay keeps
    // pushing events to. Captured so we can assert the worker returns instead
    // of pinning on a parked read loop behind a dead writer.
    let (mut ui_client, ui_handle) =
        spawn_relay_connection_capturing(&configuration_root, &bundle_paths);
    let ui_read = ui_client.try_clone().expect("clone ui stream");
    let mut ui_reader = BufReader::new(ui_read);
    send_json(&mut ui_client, hello_frame_for("panel", bundle_name));
    assert_eq!(read_json(&mut ui_reader)["frame"], "hello_ack");
    let rcvbuf_bytes = minimize_receive_buffer(&ui_client);
    // Stop draining: every event the relay now pushes accumulates unread.
    drop(ui_reader);

    // Size the flooded message to the shrunk receive buffer so a single pushed
    // incoming_message event overruns it: the relay-to-client write then blocks
    // past the 300 ms timeout almost immediately, rather than needing hundreds
    // of tiny events to fill the buffer. A 4 KiB floor covers platforms whose
    // minimum SO_RCVBUF getsockopt reports something small.
    let large_message = "x".repeat(rcvbuf_bytes.max(4096));

    // Sender: floods chat messages at the idle UI member so the relay pushes
    // one incoming_message event per send onto the UI stream. Once the shrunk
    // buffer fills, the relay-to-client write blocks past the 300 ms timeout,
    // the writer task exits, and the connection must tear down.
    let (mut sender_client, sender_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let sender_read = sender_client.try_clone().expect("clone sender stream");
    let mut sender_reader = BufReader::new(sender_read);
    send_json(&mut sender_client, agent_hello_frame(bundle_name));
    assert_eq!(read_json(&mut sender_reader)["frame"], "hello_ack");

    for index in 0..64 {
        if ui_handle.is_finished() {
            break;
        }
        send_json(
            &mut sender_client,
            json!({
                "frame": "request",
                "request_id": format!("flood-{index}"),
                "request": {
                    "operation": "send",
                    "requester_session": "alpha",
                    "message": large_message,
                    "targets": [format!("panel@{bundle_name}")],
                    "broadcast": false,
                },
            }),
        );
        // Drain the send response so the sender's own buffer never backs up.
        let _ = read_json(&mut sender_reader);
    }

    // Without the writer-arm teardown the parked UI read loop would never see
    // the dead writer and `join_within` would panic on the pinned worker.
    let outcome = join_within(ui_handle, Duration::from_secs(5));
    assert!(
        outcome.is_ok(),
        "idle UI write-timeout teardown should be a clean exit: {outcome:?}"
    );

    shutdown_stream(&sender_client, "shutdown sender client");
    sender_handle.join().expect("join sender relay thread");

    // The torn-down connection must have released its registry entry: the same
    // UI identity reconnects without an identity-claim conflict.
    let (mut reconnect_client, reconnect_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let reconnect_read = reconnect_client
        .try_clone()
        .expect("clone reconnect stream");
    let mut reconnect_reader = BufReader::new(reconnect_read);
    send_json(&mut reconnect_client, hello_frame_for("panel", bundle_name));
    assert_eq!(read_json(&mut reconnect_reader)["frame"], "hello_ack");
    shutdown_stream(&reconnect_client, "shutdown reconnect client");
    reconnect_handle
        .join()
        .expect("join reconnect relay thread");
}

// The `user@GLOBAL` (TUI) variant of the idle-UI teardown above. The original
// production regression was a relay-wide operator stream, not a bundle-local UI
// member. The teardown mechanism is RegistryKey-independent, so this asserts the
// same clean worker release and registry release for a relay-wide principal,
// guarding the `@GLOBAL` UI permission and snapshot-delivery routing paths
// against a future regression specific to relay-wide connections. See
// todos/relay/65.
#[test]
fn idle_global_operator_stream_write_timeout_tears_down_connection() {
    ensure_fast_write_timeout_for_tests();
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_idle_global_ui_timeout";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    // Declare the relay-wide operator (`@GLOBAL`) whose idle UI stream the relay
    // keeps pushing events to: the TUI connection shape from the regression.
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    // Operator UI client: register as a relay-wide stream, read the ack, shrink
    // its receive buffer, then go idle -- it never sends another frame and never
    // reads again. Captured so the worker's return is observable instead of
    // pinning on a parked read loop behind a dead writer.
    let (mut operator_client, operator_handle) =
        spawn_relay_connection_capturing(&configuration_root, &bundle_paths);
    let operator_read = operator_client.try_clone().expect("clone operator stream");
    let mut operator_reader = BufReader::new(operator_read);
    send_json(
        &mut operator_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut operator_reader)["frame"], "hello_ack");
    let rcvbuf_bytes = minimize_receive_buffer(&operator_client);
    // Stop draining: every event the relay now pushes accumulates unread.
    drop(operator_reader);

    // Size the message so a single pushed incoming_message event overruns the
    // shrunk buffer, blocking the relay-to-client write past the 300 ms timeout.
    let large_message = "x".repeat(rcvbuf_bytes.max(4096));

    // Sender: a bundle-bound session floods chat messages at the idle operator by
    // its `@GLOBAL` id, so the relay pushes one incoming_message event per send
    // onto the relay-wide stream until the shrunk buffer fills and the write
    // times out, the writer task exits, and the connection must tear down.
    let (mut sender_client, sender_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let sender_read = sender_client.try_clone().expect("clone sender stream");
    let mut sender_reader = BufReader::new(sender_read);
    send_json(&mut sender_client, agent_hello_frame(bundle_name));
    assert_eq!(read_json(&mut sender_reader)["frame"], "hello_ack");

    for index in 0..64 {
        if operator_handle.is_finished() {
            break;
        }
        send_json(
            &mut sender_client,
            json!({
                "frame": "request",
                "request_id": format!("flood-{index}"),
                "request": {
                    "operation": "send",
                    "requester_session": "alpha",
                    "message": large_message,
                    "targets": [operator_id],
                    "broadcast": false,
                },
            }),
        );
        // Drain the send response so the sender's own buffer never backs up.
        let _ = read_json(&mut sender_reader);
    }

    // Without the writer-arm teardown the parked operator read loop would never
    // see the dead writer and `join_within` would panic on the pinned worker.
    let outcome = join_within(operator_handle, Duration::from_secs(5));
    assert!(
        outcome.is_ok(),
        "idle global operator write-timeout teardown should be a clean exit: {outcome:?}"
    );

    shutdown_stream(&sender_client, "shutdown sender client");
    sender_handle.join().expect("join sender relay thread");

    // The torn-down connection must have released its registry entry: the same
    // `@GLOBAL` identity reconnects without an identity-claim conflict.
    let (mut reconnect_client, reconnect_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let reconnect_read = reconnect_client
        .try_clone()
        .expect("clone reconnect stream");
    let mut reconnect_reader = BufReader::new(reconnect_read);
    send_json(
        &mut reconnect_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut reconnect_reader)["frame"], "hello_ack");
    shutdown_stream(&reconnect_client, "shutdown reconnect client");
    reconnect_handle
        .join()
        .expect("join reconnect relay thread");
}

// Drives a bounded drain wait on a private current-thread runtime so the
// synchronous test can assert on the report.
fn block_on_drain(
    coordinator: &ConnectionDrainCoordinator,
    timeout: Duration,
) -> ConnectionDrainReport {
    TokioRuntimeBuilder::new_current_thread()
        .enable_time()
        .build()
        .expect("build current-thread runtime")
        .block_on(coordinator.wait_for_drain(timeout))
}

#[test]
fn shutdown_signal_drains_parked_connection_worker() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_drain_parked";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // The slot is registered with a coordinator the test keeps, so the test
    // plays the relay host's shutdown role against a real connection worker.
    let coordinator = ConnectionDrainCoordinator::new();
    let worker_slot = coordinator.register_worker();
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.clone();
    let state = bundle_paths.state_root.clone();
    let catalog = super::single_bundle_catalog(&bundle_paths);
    let join_handle = thread::spawn(move || {
        run_serve_connection_with_slot(server_stream, root, state, catalog, false, worker_slot)
    });

    let mut client_stream = client_stream;
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(&mut client_stream, agent_hello_frame(bundle_name));
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    // The worker is now parked on its long-lived post-hello stream read with
    // no incoming frame and no EOF; only the cooperative signal can release it.
    coordinator.signal_shutdown();
    let outcome = join_within(join_handle, Duration::from_secs(5));
    assert!(outcome.is_ok(), "drain exit should be clean: {outcome:?}");

    let report = block_on_drain(&coordinator, Duration::from_millis(500));
    assert!(
        !report.timed_out,
        "drained worker must not report a timeout"
    );
    assert_eq!(report.drained_worker_count, 1);
    assert_eq!(report.remaining_worker_count, 0);

    // The drained worker closed the connection: the client sees EOF.
    let mut line = String::new();
    let read = reader.read_line(&mut line).expect("read after drain");
    assert_eq!(read, 0, "client should observe EOF after worker drains");
}

#[test]
fn drain_wait_reports_timeout_for_undrained_workers() {
    let coordinator = ConnectionDrainCoordinator::new();
    let parked_slot = coordinator.register_worker();
    let serving_slot = coordinator.register_worker();
    let serving_guard = serving_slot.begin_serving();

    // Neither worker exits, so the bounded wait must end at its timeout with
    // a deterministic report distinguishing the serving worker from the
    // parked one.
    coordinator.signal_shutdown();
    let started = Instant::now();
    let report = block_on_drain(&coordinator, Duration::from_millis(100));
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(report.timed_out, "undrained workers must report a timeout");
    assert_eq!(report.drained_worker_count, 0);
    assert_eq!(report.remaining_worker_count, 2);
    assert_eq!(report.remaining_serving_count, 1);

    // Both workers exit after the signal: a follow-up wait reports full drain.
    drop(serving_guard);
    drop(serving_slot);
    drop(parked_slot);
    let report = block_on_drain(&coordinator, Duration::from_millis(100));
    assert!(!report.timed_out);
    assert_eq!(report.drained_worker_count, 2);
    assert_eq!(report.remaining_worker_count, 0);
    assert_eq!(report.remaining_serving_count, 0);
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
