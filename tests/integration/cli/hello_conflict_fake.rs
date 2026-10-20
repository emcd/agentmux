use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// Rejects every hello with `runtime_identity_claim_conflict` until the
/// deadline, then exits. Lets a CLI surface prove it names an exhausted
/// hello-claim retry instead of reporting saturation. Unlike
/// `spawn_fake_relay_once` this fake never acknowledges a hello, so the
/// client retries under its own conflict budget; the socket stays bound for
/// the whole window and each rejected principal id is recorded.
pub(super) fn spawn_fake_relay_with_hello_conflict(
    socket_path: &Path,
    rejected: Arc<Mutex<Vec<String>>>,
) -> thread::JoinHandle<()> {
    if socket_path.exists() {
        fs::remove_file(socket_path).expect("remove stale relay socket");
    }
    let parent = socket_path.parent().expect("relay socket parent");
    fs::create_dir_all(parent).expect("create relay socket parent");
    let listener =
        UnixListener::bind(socket_path).expect("bind fake relay socket");
    listener
        .set_nonblocking(true)
        .expect("set fake relay listener nonblocking");
    let socket_path = socket_path.to_path_buf();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _address)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("set accepted stream blocking");
                    let mut reader = BufReader::new(
                        stream.try_clone().expect("clone fake relay stream"),
                    );
                    let mut hello_line = String::new();
                    reader
                        .read_line(&mut hello_line)
                        .expect("read fake relay hello");
                    let hello: Value =
                        serde_json::from_str(hello_line.trim_end())
                            .expect("decode fake relay hello");
                    assert_eq!(
                        hello.get("frame").and_then(Value::as_str),
                        Some("hello")
                    );
                    if let Some(principal_id) =
                        hello.get("principal_id").and_then(Value::as_str)
                    {
                        rejected
                            .lock()
                            .expect("rejected lock")
                            .push(principal_id.to_string());
                    }
                    let conflict = json!({
                        "frame": "response",
                        "response": {
                            "kind": "error",
                            "error": {
                                "code": "runtime_identity_claim_conflict",
                                "message": "stream identity is already claimed by a live connection",
                            },
                        },
                    });
                    let encoded = serde_json::to_string(&conflict)
                        .expect("encode fake relay conflict");
                    stream
                        .write_all(encoded.as_bytes())
                        .expect("write fake relay conflict");
                    stream.write_all(b"\n").expect("write fake relay newline");
                    stream.flush().expect("flush fake relay conflict");
                },
                Err(source)
                    if source.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    thread::sleep(Duration::from_millis(10));
                },
                Err(source) => {
                    panic!("accept fake relay connection: {source}")
                },
            }
        }
        let _ = fs::remove_file(socket_path);
    })
}
