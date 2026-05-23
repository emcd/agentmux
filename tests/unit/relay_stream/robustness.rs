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
    let paths = bundle_paths.clone();
    let join_handle = thread::spawn(move || run_serve_connection(server_stream, root, paths));
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
