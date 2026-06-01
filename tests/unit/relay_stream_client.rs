use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use agentmux::relay::RelayStreamSession;
use serde_json::{Value, json};
static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

struct SocketPathGuard {
    socket_path: PathBuf,
}

impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn temporary_socket_path(prefix: &str) -> (SocketPathGuard, PathBuf) {
    let counter = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let short_prefix = &prefix[..prefix.len().min(12)];
    let socket_path =
        PathBuf::from("/tmp").join(format!("amx-{short_prefix}-{pid}-{counter}.sock"));
    let _ = std::fs::remove_file(&socket_path);
    (
        SocketPathGuard {
            socket_path: socket_path.clone(),
        },
        socket_path,
    )
}

fn read_json_line(reader: &mut BufReader<std::os::unix::net::UnixStream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read json line");
    serde_json::from_str(line.trim_end()).expect("decode json line")
}

fn write_json_line(stream: &mut std::os::unix::net::UnixStream, value: &Value) {
    let text = serde_json::to_string(value).expect("encode json line");
    stream
        .write_all(format!("{text}\n").as_bytes())
        .expect("write json line");
    stream.flush().expect("flush json line");
}

fn shutdown_stream(stream: &std::os::unix::net::UnixStream, context: &str) {
    match stream.shutdown(std::net::Shutdown::Both) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotConnected => {}
        Err(source) => panic!("{context}: {source:?}"),
    }
}

fn assert_and_ack_hello(
    reader: &mut BufReader<std::os::unix::net::UnixStream>,
    stream: &mut std::os::unix::net::UnixStream,
    bundle_name: &str,
    session_id: &str,
) {
    let principal_id = if session_id.ends_with("@GLOBAL") {
        session_id.to_string()
    } else {
        format!("{session_id}@{bundle_name}")
    };
    let hello_payload = read_json_line(reader);
    assert_eq!(hello_payload["frame"], "hello");
    assert_eq!(hello_payload["principal_id"], principal_id);
    assert!(
        hello_payload["identity_token"].is_string(),
        "hello frame must carry an identity_token",
    );
    write_json_line(
        stream,
        &json!({
            "frame": "hello_ack",
            "schema_version": "1",
            "principal_id": principal_id,
        }),
    );
}

#[test]
fn stream_client_poll_events_returns_pending_event_frames() {
    let (_temporary, socket_path) = temporary_socket_path("relay-stream-client-events");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept client");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        assert_and_ack_hello(&mut reader, &mut stream, "party", "alpha");

        thread::sleep(Duration::from_millis(80));
        let event = json!({
            "frame": "event",
            "event": {
                "event_type": "incoming_message",
                "bundle_name": "party",
                "target_session": "alpha",
                "created_at": "2026-03-18T00:00:00Z",
                "payload": {
                    "message_id": "msg-1",
                    "sender_session": "master",
                    "body": "hello"
                }
            }
        });
        let event_text = serde_json::to_string(&event).expect("encode event");
        stream
            .write_all(format!("{event_text}\n").as_bytes())
            .expect("write event");
        stream.flush().expect("flush event");
        thread::sleep(Duration::from_millis(200));
    });

    let mut session =
        RelayStreamSession::new(socket_path, "party".to_string(), "alpha".to_string());

    let deadline = Instant::now() + Duration::from_millis(750);
    let received = loop {
        let events = session.poll_events().expect("poll events");
        if !events.is_empty() {
            break events;
        }
        assert!(
            Instant::now() < deadline,
            "expected relay stream event before timeout",
        );
        thread::sleep(Duration::from_millis(25));
    };

    assert_eq!(received.len(), 1);
    assert_eq!(received[0].event_type, "incoming_message");
    assert_eq!(received[0].bundle_name, "party");
    assert_eq!(received[0].target_session, "alpha");
    assert_eq!(received[0].payload["sender_session"], "master");
    server.join().expect("join server thread");
}

#[test]
fn stream_client_does_not_auto_retry_request_after_disconnect() {
    let (_temporary, socket_path) = temporary_socket_path("relay-stream-client-no-auto-retry");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    let server = thread::spawn(move || {
        // First stream: accept hello + request, then close before response.
        let (mut first_stream, _) = listener.accept().expect("accept first client");
        let mut first_reader = BufReader::new(first_stream.try_clone().expect("clone first"));
        assert_and_ack_hello(&mut first_reader, &mut first_stream, "party", "alpha");
        let first_request = read_json_line(&mut first_reader);
        assert_eq!(first_request["frame"], "request");
        assert_eq!(first_request["request"]["operation"], "list");
        shutdown_stream(&first_stream, "shutdown first stream");

        // Second stream: fresh hello + request, then normal response.
        let (mut second_stream, _) = listener.accept().expect("accept second client");
        let mut second_reader = BufReader::new(second_stream.try_clone().expect("clone second"));
        assert_and_ack_hello(&mut second_reader, &mut second_stream, "party", "alpha");
        let second_request = read_json_line(&mut second_reader);
        assert_eq!(second_request["frame"], "request");
        assert_eq!(second_request["request"]["operation"], "list");

        let request_id = second_request["request_id"]
            .as_str()
            .map(ToOwned::to_owned)
            .expect("request id");
        write_json_line(
            &mut second_stream,
            &json!({
                "frame": "response",
                "request_id": request_id,
                "response": {
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "party",
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [],
                    },
                }
            }),
        );
    });

    let mut session =
        RelayStreamSession::new(socket_path, "party".to_string(), "alpha".to_string());
    let first_error = session
        .request_with_events(&agentmux::relay::RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        })
        .expect_err("disconnect should fail first request");
    assert_eq!(first_error.kind(), std::io::ErrorKind::UnexpectedEof);

    let (response, events) = session
        .request_with_events(&agentmux::relay::RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        })
        .expect("second request should reconnect and succeed");
    assert!(events.is_empty());
    match response {
        agentmux::relay::RelayResponse::List { bundle, .. } => {
            assert_eq!(bundle.id, "party");
            assert!(bundle.sessions.is_empty());
        }
        other => panic!("unexpected response: {other:?}"),
    }
    server.join().expect("join server");
}

#[test]
fn stream_client_retries_hello_after_identity_claim_conflict() {
    let (_temporary, socket_path) =
        temporary_socket_path("relay-stream-client-hello-conflict-retry");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    let server = thread::spawn(move || {
        let (mut conflict_stream, _) = listener.accept().expect("accept conflict client");
        let mut conflict_reader = BufReader::new(conflict_stream.try_clone().expect("clone"));
        let hello_payload = read_json_line(&mut conflict_reader);
        assert_eq!(hello_payload["frame"], "hello");
        write_json_line(
            &mut conflict_stream,
            &json!({
                "frame": "response",
                "response": {
                    "kind": "error",
                    "error": {
                        "code": "runtime_identity_claim_conflict",
                        "message": "stream identity is already claimed by a live connection"
                    }
                }
            }),
        );
        shutdown_stream(&conflict_stream, "shutdown conflict stream");

        let (mut stream, _) = listener.accept().expect("accept retry client");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        assert_and_ack_hello(&mut reader, &mut stream, "party", "alpha");

        let request = read_json_line(&mut reader);
        assert_eq!(request["frame"], "request");
        assert_eq!(request["request"]["operation"], "list");
        let request_id = request["request_id"]
            .as_str()
            .map(ToOwned::to_owned)
            .expect("request id");
        write_json_line(
            &mut stream,
            &json!({
                "frame": "response",
                "request_id": request_id,
                "response": {
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "party",
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [],
                    },
                }
            }),
        );
    });

    let mut session =
        RelayStreamSession::new(socket_path, "party".to_string(), "alpha".to_string());
    let (response, events) = session
        .request_with_events(&agentmux::relay::RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        })
        .expect("retry after hello conflict should succeed");
    assert!(events.is_empty());
    match response {
        agentmux::relay::RelayResponse::List { bundle, .. } => {
            assert_eq!(bundle.id, "party");
            assert!(bundle.sessions.is_empty());
        }
        other => panic!("unexpected response: {other:?}"),
    }
    server.join().expect("join server");
}

#[test]
fn stream_client_reports_exhausted_hello_conflict_as_timeout() {
    let (_temporary, socket_path) =
        temporary_socket_path("relay-stream-client-hello-conflict-exhausted");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    let done = Arc::new(AtomicBool::new(false));
    let server_done = Arc::clone(&done);

    // Answer every hello with an identity-claim conflict so the client retries
    // until its hello-conflict deadline elapses. The `done` flag plus a
    // wake-up connection let the accept loop exit once the client gives up.
    let server = thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            if server_done.load(Ordering::Relaxed) {
                break;
            }
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let hello_payload = read_json_line(&mut reader);
            assert_eq!(hello_payload["frame"], "hello");
            write_json_line(
                &mut stream,
                &json!({
                    "frame": "response",
                    "response": {
                        "kind": "error",
                        "error": {
                            "code": "runtime_identity_claim_conflict",
                            "message": "stream identity is already claimed by a live connection"
                        }
                    }
                }),
            );
            shutdown_stream(&stream, "shutdown conflict stream");
        }
    });

    let mut session = RelayStreamSession::new(
        socket_path.clone(),
        "party".to_string(),
        "alpha".to_string(),
    );
    let error = session
        .request_with_events(&agentmux::relay::RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        })
        .expect_err("persistent hello conflict should fail the request");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::TimedOut,
        "exhausted hello conflict must surface as a timeout: {error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("runtime_identity_claim_conflict"),
        "timeout error should embed the conflict cause: {error}"
    );

    // Release the server's accept loop now that the client has stopped.
    done.store(true, Ordering::Relaxed);
    let _ = UnixStream::connect(&socket_path);
    server.join().expect("join server");
}

#[test]
fn stream_client_detects_idle_disconnect_and_reconnects_on_next_request() {
    // Verifies the liveness check in `ensure_connected`: after the relay
    // closes the connection between requests, the next request must observe
    // the dead socket via the non-blocking peek and reconnect transparently
    // in the SAME call, rather than writing into a half-closed socket and
    // blocking on the 5-second response timeout.
    let (_temporary, socket_path) = temporary_socket_path("relay-stream-client-idle-drop");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    let server = thread::spawn(move || {
        // First stream: hello, one list request, full response, then shutdown.
        let (mut first_stream, _) = listener.accept().expect("accept first client");
        let mut first_reader = BufReader::new(first_stream.try_clone().expect("clone first"));
        assert_and_ack_hello(&mut first_reader, &mut first_stream, "party", "alpha");
        let first_request = read_json_line(&mut first_reader);
        let first_request_id = first_request["request_id"]
            .as_str()
            .map(ToOwned::to_owned)
            .expect("first request id");
        write_json_line(
            &mut first_stream,
            &json!({
                "frame": "response",
                "request_id": first_request_id,
                "response": {
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "party",
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [],
                    },
                }
            }),
        );
        shutdown_stream(&first_stream, "shutdown first stream after response");

        // Second stream: a fresh hello + request must arrive on the next call.
        let (mut second_stream, _) = listener.accept().expect("accept second client");
        let mut second_reader = BufReader::new(second_stream.try_clone().expect("clone second"));
        assert_and_ack_hello(&mut second_reader, &mut second_stream, "party", "alpha");
        let second_request = read_json_line(&mut second_reader);
        let second_request_id = second_request["request_id"]
            .as_str()
            .map(ToOwned::to_owned)
            .expect("second request id");
        write_json_line(
            &mut second_stream,
            &json!({
                "frame": "response",
                "request_id": second_request_id,
                "response": {
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "party",
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [],
                    },
                }
            }),
        );
    });

    let mut session =
        RelayStreamSession::new(socket_path, "party".to_string(), "alpha".to_string());
    let (first_response, _) = session
        .request_with_events(&agentmux::relay::RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        })
        .expect("first request should succeed");
    match first_response {
        agentmux::relay::RelayResponse::List { bundle, .. } => assert_eq!(bundle.id, "party"),
        other => panic!("unexpected first response: {other:?}"),
    }

    // Wait briefly so the server's shutdown propagates to the client's
    // socket recv path before the next `ensure_connected` call peeks.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }

    let started_at = Instant::now();
    let (second_response, _) = session
        .request_with_events(&agentmux::relay::RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        })
        .expect("second request should reconnect transparently on the SAME call");
    let elapsed = started_at.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "liveness check must trigger reconnect well before the response timeout; took {elapsed:?}",
    );
    match second_response {
        agentmux::relay::RelayResponse::List { bundle, .. } => assert_eq!(bundle.id, "party"),
        other => panic!("unexpected second response: {other:?}"),
    }
    server.join().expect("join server");
}
