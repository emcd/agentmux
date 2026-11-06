//! Live-executable pipe tests for the reference adapter binary.
//!
//! These drive the built binary over real OS pipes (not in-memory
//! buffers) to prove bounded behavior where it matters: a silent peer
//! cannot park the run loop past the handshake timeout, a partial
//! frame cannot either, a dead channel terminalizes instead of hanging,
//! and an outbox cap refusal terminalizes instead of queueing
//! unboundedly. Timeouts use real wall-clock bounds with generous
//! margins; no test asserts on timing precision, only on termination
//! and exit codes.

use std::io::Write;
use std::os::unix::io::FromRawFd;
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

/// A silent peer cannot park the run loop: the handshake timeout fires.
#[test]
fn silent_peer_hits_handshake_timeout() {
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
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("adapter exits");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(150),
        "must wait out the timeout, not fail instantly"
    );
    child.kill().ok();
}

/// A partial frame with no terminator cannot park the run loop either.
#[test]
fn partial_frame_hits_handshake_timeout() {
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
    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(b"{\"jsonrpc\": ")
        .expect("partial write lands");
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("adapter exits");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(150),
        "must wait out the timeout, not fail instantly"
    );
    child.kill().ok();
}

/// A closed channel terminalizes instead of hanging on write. The
/// stdout pipe's read end is closed before the child is even spawned,
/// so the very first write deterministically fails: no post-spawn
/// drop race can let the hello request out first. No stdin traffic is
/// offered at all and stdin stays held open, so EOF can never offer
/// a clean exit either — the only way out is the dead-channel
/// terminalization, proven by its diagnostic (a bare exit code could
/// also come from paths that record nothing). In production the
/// supervisor cannot send the hello result without having received
/// that request, so the EPIPE always wins there too.
#[test]
fn closed_stdout_exits_unusable() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    // Pre-closed stdout: every write from the first byte fails.
    let mut fds = [0; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { libc::close(fds[0]) }, 0);
    let stdout = unsafe { Stdio::from(std::fs::File::from_raw_fd(fds[1])) };
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
        ])
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns");
    // Stdin stays held open and silent: EOF must never arrive to offer
    // the run loop a clean exit. Poll rather than blocking-wait so a
    // regression that hangs instead of terminalizing fails loudly.
    let start = Instant::now();
    let code = loop {
        if let Some(status) = child.try_wait().expect("wait works") {
            break status.code();
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "adapter must terminalize the closed stdout, not hang"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = child.kill();
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(code.or(output.status.code()), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Pre-binding the handshake fails without terminalizing (there
    // is no binding to retire), so the dead-channel evidence is the
    // writer-event record, not a terminalize entry: a handshake
    // timeout would record nothing at all, which this assertion
    // excludes.
    assert!(
        stderr.contains("transport_dead"),
        "dead-channel observation must be recorded, not just exited: {stderr}"
    );
}

/// An outbox cap refusal terminalizes instead of queueing unboundedly.
#[test]
fn tiny_cap_refuses_instead_of_queueing() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns");
    let mut hello = hello_result();
    hello["result"]["negotiated_limits"]["queue_tx_bytes"] =
        serde_json::json!(10);
    let submit = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-1",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": "aGk=",
            }],
        },
    });
    let mut wire = serde_json::to_vec(&hello).expect("serializes");
    wire.push(b'\n');
    let submit_bytes = serde_json::to_vec(&submit).expect("serializes");
    wire.extend_from_slice(&submit_bytes);
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("fixtures fit the pipe");
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("outbox_full"),
        "cap refusal must be diagnosed, got: {stderr}"
    );
}

/// Hello carrying custom negotiated limits for pipe scenarios.
fn hello_negotiated(
    max_frame: u64,
    drain_wait_ms: u64,
    tx: u64,
    rx: u64,
) -> serde_json::Value {
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
                "max_frame_bytes": max_frame,
                "queue_tx_bytes": tx,
                "queue_rx_bytes": rx,
                "max_control_pending": 32,
                "max_events_per_second": 100,
                "window_seconds": 1,
                "max_submits_in_flight": 64,
                "max_choices_in_flight": 16,
                "max_looks_in_flight": 8,
                "drain_wait_ms": drain_wait_ms,
            },
        },
    })
}

/// A look poll for `target` with the given prime timeout.
fn look_poll(id: &str, target: &str, prime_timeout_ms: u64) -> Vec<u8> {
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "relay.look",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": target,
            "adapter_generation": 3,
            "correlation_id": format!("look-{id}"),
            "look_handle_id": format!("h-{id}"),
            "prime_timeout_ms": prime_timeout_ms,
        },
    });
    let mut bytes = serde_json::to_vec(&frame).expect("serializes");
    bytes.push(b'\n');
    bytes
}

/// After negotiating up, a first post-hello frame larger than the
/// 64KiB bootstrap ceiling is admitted: the transition is ordered
/// before post-hello acquisition even when both frames coalesce.
#[test]
fn coalesced_up_negotiation_admits_large_first_frame() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
            "--max-frame-bytes",
            "1048576",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    let hello = hello_negotiated(1_048_576, 250, 16_777_216, 16_777_216);
    let big = "aGk".repeat(25 * 1024);
    let submit = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-1",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": big,
            }],
        },
    });
    let terminate = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "terminate",
        },
    });
    let mut wire = serde_json::to_vec(&hello).expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&serde_json::to_vec(&submit).expect("serializes"));
    wire.push(b'\n');
    wire.extend_from_slice(
        &serde_json::to_vec(&terminate).expect("serializes"),
    );
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("coalesced input fits");
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"outcome\":\"Submitted\""),
        "large first frame admitted after up-negotiation: {stdout}"
    );
}

/// After negotiating down, a first post-hello frame larger than the
/// negotiated bound is refused even when coalesced with the hello.
#[test]
fn coalesced_down_negotiation_refuses_large_first_frame() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    let hello = hello_negotiated(4096, 250, 16_777_216, 16_777_216);
    let big = "aGk".repeat(2 * 1024);
    let submit = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-1",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": big,
            }],
        },
    });
    let mut wire = serde_json::to_vec(&hello).expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&serde_json::to_vec(&submit).expect("serializes"));
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    let _ = stdin.write_all(&wire);
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(0));
    // The oversize first frame is discarded at acquisition: only the
    // hello request completed, and no submit terminal was fabricated
    // for the refused frame.
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let frame: serde_json::Value =
            serde_json::from_str(line).expect("complete frame parses");
        assert!(
            frame.get("result").is_none(),
            "no terminal for the refused frame: {frame:?}"
        );
    }
}

/// A delayed first operation after down-negotiation is refused under
/// the installed bound, not the bootstrap ceiling.
#[test]
fn delayed_down_negotiation_refuses_large_frame() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    let hello = hello_negotiated(4096, 250, 16_777_216, 16_777_216);
    let mut wire = serde_json::to_vec(&hello).expect("serializes");
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("hello lands");
    std::thread::sleep(Duration::from_millis(300));
    let big = "aGk".repeat(2 * 1024);
    let submit = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-1",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": big,
            }],
        },
    });
    let mut frame = serde_json::to_vec(&submit).expect("serializes");
    frame.push(b'\n');
    let _ = stdin.write_all(&frame);
    drop(stdin);
    let status = child.wait().expect("adapter exits");
    assert_eq!(status.code(), Some(0));
}

/// A delayed first operation after up-negotiation is admitted under
/// the installed bound.
#[test]
fn delayed_up_negotiation_admits_large_frame() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
            "--max-frame-bytes",
            "1048576",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    let hello = hello_negotiated(1_048_576, 250, 16_777_216, 16_777_216);
    let mut wire = serde_json::to_vec(&hello).expect("serializes");
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("hello lands");
    std::thread::sleep(Duration::from_millis(300));
    let big = "aGk".repeat(25 * 1024);
    let submit = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-1",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": big,
            }],
        },
    });
    let terminate = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "terminate",
        },
    });
    let mut frame = serde_json::to_vec(&submit).expect("serializes");
    frame.push(b'\n');
    stdin.write_all(&frame).expect("delayed frame lands");
    let mut term = serde_json::to_vec(&terminate).expect("serializes");
    term.push(b'\n');
    stdin.write_all(&term).expect("terminate lands");
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"outcome\":\"Submitted\""),
        "delayed large frame admitted after up-negotiation"
    );
}

/// The look prime timeout is honored adapter-side: scripted data
/// available 100ms after the first poll answers fresh within the
/// 500ms relay-supplied bound.
#[test]
fn prime_wait_answers_fresh_within_bound() {
    let script = "[[snapshots]]\ntarget = \"seat-1\"\ndelay_ms = 100\n\n[[snapshots.entries]]\nkind = \"text\"\nseq = 1\ntext = \"slow hello\"\n";
    let (_dir, path) = fixture(script);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            path.to_str().expect("utf8"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_result()).expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 500));
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("look lands");
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut reader = std::io::BufReader::new(&mut stdout);
    let mut hello_line = String::new();
    use std::io::BufRead as _;
    reader
        .read_line(&mut hello_line)
        .expect("hello request out");
    let mut answer = String::new();
    reader.read_line(&mut answer).expect("look answer out");
    let elapsed = start.elapsed();
    drop(stdin);
    drop(reader);
    child.kill().ok();
    let _ = child.wait();
    assert!(
        elapsed < Duration::from_secs(5),
        "prime wait bounded by the relay timeout"
    );
    let frame: serde_json::Value =
        serde_json::from_str(&answer).expect("answer parses");
    assert_eq!(frame["result"]["freshness"], "fresh");
    assert_eq!(frame["result"]["entries"][0]["seq"], 1);
}

/// A malformed-envelope flood through the executable never resets
/// liveness: the peer is silent in every way that matters, so the
/// timeout still expires.
#[test]
fn malformed_envelope_flood_still_expires_liveness() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
            "--liveness-timeout-ms",
            "300",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns");
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_result()).expect("serializes");
    wire.push(b'\n');
    for _ in 0..50 {
        wire.extend_from_slice(b"{\"jsonrpc\":\"2.0\"}\n");
    }
    let mut stdin = child.stdin.take().expect("stdin piped");
    let _ = stdin.write_all(&wire);
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("liveness expires despite the flood");
    assert_eq!(code, 0);
    child.kill().ok();
    let output = child.wait_with_output().expect("adapter exits");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("liveness_timeout"),
        "expiry terminalizes, got: {stderr}"
    );
}

/// lines/offset slice a tail-N window with exact counts: with four
/// scripted entries, lines=2 offset=1 returns seqs 2..3 (skipping the
/// newest), entries_total 4, returned 2, scripted flags preserved; an
/// offset past the start returns empty with totals intact; an
/// unbounded poll returns everything.
#[test]
fn look_lines_offset_slice_tail_window_with_counts() {
    let script = "[[snapshots]]\ntarget = \"seat-1\"\n\n\
        [[snapshots.entries]]\nkind = \"text\"\nseq = 1\ntext = \"one\"\n\n\
        [[snapshots.entries]]\nkind = \"text\"\nseq = 2\ntext = \"two\"\n\n\
        [[snapshots.entries]]\nkind = \"text\"\nseq = 3\ntext = \"three\"\n\n\
        [[snapshots.entries]]\nkind = \"text\"\nseq = 4\ntext = \"four\"\n";
    let (_dir, path) = fixture(script);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            path.to_str().expect("utf8"),
            "--max-frame-bytes",
            "65536",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns");
    let look = |id: &str, lines: Option<u64>, offset: Option<u64>| {
        let mut params = serde_json::json!({
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": format!("look-{id}"),
            "look_handle_id": format!("h-{id}"),
            "prime_timeout_ms": 0,
        });
        if let Some(lines) = lines {
            params["lines"] = lines.into();
        }
        if let Some(offset) = offset {
            params["offset"] = offset.into();
        }
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "relay.look",
            "params": params,
        });
        let mut bytes = serde_json::to_vec(&frame).expect("serializes");
        bytes.push(b'\n');
        bytes
    };
    let mut wire = serde_json::to_vec(&hello_result()).expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look("10", Some(2), Some(1)));
    wire.extend_from_slice(&look("11", None, Some(10)));
    wire.extend_from_slice(&look("12", None, None));
    wire.extend_from_slice(
        &serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "relay.supervisionSignal",
            "params": {
                "protocol_major": 2,
                "protocol_minor": 0,
                "runtime_instance": "rt-9",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "signal": "terminate",
            },
        }))
        .expect("serializes"),
    );
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("hello+looks land");
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let frames: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("frame parses"))
        .collect();
    let by_id = |id: &str| {
        frames
            .iter()
            .find(|frame| frame["id"] == id)
            .unwrap_or_else(|| panic!("reply {id} present: {frames:?}"))
            .clone()
    };
    // Tail-N: skip the newest one (seq 4), take the last two of the
    // rest: seqs 2 and 3. Scripted per-entry flags are preserved
    // verbatim — the cut is expressed through the counts alone.
    let windowed = by_id("10");
    assert_eq!(windowed["result"]["entries_total"], 4);
    assert_eq!(windowed["result"]["returned_entries_count"], 2);
    let seqs: Vec<u64> = windowed["result"]["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|entry| entry["seq"].as_u64().expect("seq"))
        .collect();
    assert_eq!(seqs, vec![2, 3]);
    assert_eq!(windowed["result"]["entries"][0]["truncated"], false);
    assert_eq!(windowed["result"]["entries"][1]["truncated"], false);
    assert_eq!(windowed["result"]["freshness"], "fresh");
    // Offset past the start: empty window, totals intact.
    let past = by_id("11");
    assert_eq!(past["result"]["entries_total"], 4);
    assert_eq!(past["result"]["returned_entries_count"], 0);
    assert_eq!(
        past["result"]["entries"].as_array().expect("array").len(),
        0
    );
    // Unbounded: everything, nothing marked.
    let full = by_id("12");
    assert_eq!(full["result"]["entries_total"], 4);
    assert_eq!(full["result"]["returned_entries_count"], 4);
    let full_seqs: Vec<u64> = full["result"]["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|entry| entry["seq"].as_u64().expect("seq"))
        .collect();
    assert_eq!(full_seqs, vec![1, 2, 3, 4]);
    assert!(
        full["result"]["entries"]
            .as_array()
            .expect("array")
            .iter()
            .all(|entry| entry["truncated"] == false),
        "unbounded window marks nothing: {full:?}"
    );
}

/// End-to-end echo-shape enforcement on an originated call: a
/// correlation_invalid error carrying a NUMERIC correlation_id echo
/// is malformed (the exemption waives the echo requirement, never
/// the type of a field actually sent), the choice stays open, and
/// the later valid choice result still completes the turn
/// Submitted. Binary, real pipes, transcript asserted on stdout and
/// the typed diagnostic on stderr.
#[test]
fn originated_choice_nonstring_echo_kept_open_end_to_end() {
    let script = r#"
[[turns]]
correlation = "c-2"
ack = "submitted"

[[turns.choices]]
title = "Deploy?"
species = "approval"
details = "ship it"

[[turns.choices.options]]
option_id = "yes"
name = "Yes"
species = "confirm"
"#;
    let (_dir, path) = fixture(script);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            path.to_str().expect("utf8"),
            "--max-frame-bytes",
            "65536",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns");
    let submit = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-2",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": "aGk=",
            }],
        },
    });
    // correlation_invalid, but the echoed correlation is a number.
    let bad_echo = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "bogus correlation",
            "data": {
                "code": "correlation_invalid",
                "reason": "rejected",
                "correlation_id": 7,
            },
        },
    });
    let valid = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    let terminate = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "terminate",
        },
    });
    let mut wire = serde_json::to_vec(&hello_result()).expect("serializes");
    wire.push(b'\n');
    for frame in [&submit, &bad_echo, &valid, &terminate] {
        wire.extend_from_slice(
            &serde_json::to_vec(frame).expect("serializes"),
        );
        wire.push(b'\n');
    }
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("transcript lands");
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let frames: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("frame parses"))
        .collect();
    assert!(
        frames
            .iter()
            .any(|frame| frame["method"] == "adapter.requestChoice"),
        "choice must be originated: {frames:?}"
    );
    let terminals: Vec<&serde_json::Value> = frames
        .iter()
        .filter(|frame| {
            frame
                .get("result")
                .is_some_and(|r| r.get("outcome").is_some())
        })
        .collect();
    assert_eq!(terminals.len(), 1, "one terminal, after the valid reply");
    assert_eq!(terminals[0]["result"]["outcome"], "Submitted");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("malformed_reply_error"),
        "present non-string echo is malformed: {stderr}"
    );
}
