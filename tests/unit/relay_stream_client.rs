use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use agentmux::relay::{RelayStreamClientClass, RelayStreamSession};
use serde_json::{Value, json};
use tempfile::TempDir;

fn temporary_socket_path(prefix: &str) -> (TempDir, PathBuf) {
    let temporary = TempDir::new().expect("temporary directory");
    let socket_path = temporary.path().join(format!("{prefix}.sock"));
    (temporary, socket_path)
}

#[test]
fn stream_client_poll_events_returns_pending_event_frames() {
    let (_temporary, socket_path) = temporary_socket_path("relay-stream-client-events");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept client");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut hello = String::new();
        reader.read_line(&mut hello).expect("read hello");
        let hello_payload: Value =
            serde_json::from_str(hello.trim_end()).expect("decode hello payload");
        assert_eq!(hello_payload["frame"], "hello");
        let hello_ack = json!({
            "frame": "hello_ack",
            "schema_version": "1",
            "bundle_name": "party",
            "session_id": "alpha",
            "client_class": "ui",
        });
        let hello_ack_text = serde_json::to_string(&hello_ack).expect("encode hello ack");
        stream
            .write_all(format!("{hello_ack_text}\n").as_bytes())
            .expect("write hello ack");
        stream.flush().expect("flush hello ack");

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

    let mut session = RelayStreamSession::new(
        socket_path,
        "party".to_string(),
        "alpha".to_string(),
        RelayStreamClientClass::Ui,
    );

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
