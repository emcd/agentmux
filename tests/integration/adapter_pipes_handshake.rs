//! Live-executable handshake-timeout pipe tests: the run loop
//! cannot be parked past the handshake deadline by silence, partial
//! frames, wrong-id floods, or late matching hellos. Each
//! integration file owns its fixture helpers outright.
//!
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("test executable path");
    path.pop();
    if path.file_name().and_then(|name| name.to_str()) == Some("deps") {
        path.pop();
    }
    path.join("agentmux-adapter")
}

fn hello_result() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        "result": {
            "runtime_instance": "rt-9",
            "adapter_generation": 3,
            "selected_major": 2,
            "selected_minor": 0,
            "snapshot_b64": "",
            "negotiated_limits": {
                "max_frame_bytes": 65536,
                "queue_tx_bytes": 16777216,
                "queue_rx_bytes": 16777216,
                "max_control_pending": 32,
                "max_events_per_second": 100,
                "window_seconds": 1,
                "max_submits_in_flight": 64,
                "max_choices_in_flight": 16,
                "max_looks_in_flight": 8,
                "drain_wait_ms": 250,
            },
        },
    })
}

fn fixture(script: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("adapter.toml");
    std::fs::write(&path, script).expect("fixture writes");
    (dir, path)
}

const MINIMAL_SCRIPT: &str = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;

/// Polls for child exit, returning the code once observed.
/// Uses try_wait rather than wait: in this environment a plain wait on
/// a child with silent piped stdin returns immediately as if at EOF,
/// which would turn a silent peer into an EOF peer. Polling keeps the
/// pipe open so timeouts are actually exercised.
fn poll_exit(
    child: &mut std::process::Child,
    start: std::time::Instant,
    limit: Duration,
) -> Option<i32> {
    loop {
        if let Some(status) = child.try_wait().expect("wait works") {
            return status.code();
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A flood of parseable-but-wrong prebind frames cannot postpone the
/// handshake deadline: every iteration checks the absolute deadline
/// independent of receive outcome, so continuous wrong-id traffic
/// still expires at the same 200ms timeout the quiet peer observes
/// instead of parking the handshake until the traffic stops.
#[test]
fn wrong_id_flood_does_not_postpone_handshake_timeout() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let start = Instant::now();
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
            "--handshake-timeout-ms",
            "200",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    let mut stdin = child.stdin.take().expect("stdin piped");
    // Wrong-id frames every 10ms for 600ms: parseable, non-matching,
    // discarded with the reader gate released each time.
    let flooder = std::thread::spawn(move || {
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "999",
            "result": {"runtime_instance": "rt-9", "adapter_generation": 3},
        });
        let mut bytes = serde_json::to_vec(&frame).expect("serializes");
        bytes.push(b'\n');
        let end = Instant::now() + Duration::from_millis(600);
        while Instant::now() < end {
            if stdin.write_all(&bytes).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("handshake expires despite the flood");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(150),
        "must wait out the timeout, not fail instantly"
    );
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "the deadline fires on its own, not when traffic stops"
    );
    flooder.join().ok();
    child.kill().ok();
}

/// A matching hello that arrives after the deadline is already
/// expired: the test syncs on the hello request (handshake start),
/// lands a wrong-id frame at +960ms to start a fresh 50ms receive
/// slice spanning the 1000ms deadline, and delivers the matching
/// hello at +1010ms inside that slice. The post-receipt deadline
/// check rejects it — a slice in flight never smuggles a late hello
/// past the timeout. Exit is refusal with only the hello request on
/// the wire.
#[test]
fn overdue_matching_hello_is_rejected_not_accepted() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let start = Instant::now();
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
            "--handshake-timeout-ms",
            "1000",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    let mut stdin = child.stdin.take().expect("stdin piped");
    let mut stdout = child.stdout.take().expect("stdout piped");
    // Sync on the hello request: it marks the handshake start the
    // deadline counts from.
    use std::io::{BufRead, BufReader};
    let mut hello_line = String::new();
    BufReader::new(&mut stdout)
        .read_line(&mut hello_line)
        .expect("hello request arrives");
    assert!(
        hello_line.contains("adapter.handshakeHello"),
        "first frame is the hello request: {hello_line}"
    );
    let wrong = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "999",
        "result": {"runtime_instance": "rt-9", "adapter_generation": 3},
    });
    let mut wrong_bytes = serde_json::to_vec(&wrong).expect("serializes");
    wrong_bytes.push(b'\n');
    // Land the wrong-id inside the final 50ms window so the next
    // receive slice spans the deadline ...
    std::thread::sleep(Duration::from_millis(975));
    stdin.write_all(&wrong_bytes).expect("wrong-id lands late");
    // ... and the matching hello inside that spanning slice, overdue.
    // Only the post-receipt deadline check can refuse it: the
    // loop-top check already passed for this slice, and the slice
    // itself does not end before the hello arrives.
    std::thread::sleep(Duration::from_millis(35));
    let mut hello = serde_json::to_vec(&hello_result()).expect("serializes");
    hello.push(b'\n');
    // The peer may already be gone: a refused write is fine.
    let _ = stdin.write_all(&hello);
    drop(stdin);
    drop(stdout);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "the overdue hello must fail fast, not hang"
    );
    // The hello request went to the taken stdout handle; nothing
    // else may follow it — the late match is refused, not answered.
    assert_eq!(
        output.stdout.lines().count(),
        0,
        "no further frames after the refused match"
    );
}
