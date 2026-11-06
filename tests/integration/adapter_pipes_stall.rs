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

fn fixture(script: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("adapter.toml");
    std::fs::write(&path, script).expect("fixture writes");
    (dir, path)
}

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

/// A single blocked frame observes the drain wait: with stdout
/// undrained and stdin held open, one stalled 2MiB look answer
/// terminalizes the session within the negotiated bound — no queue
/// filling, no EOF, and no liveness expiry required.
#[test]
fn single_blocked_frame_observes_drain_wait() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &[]);
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 250, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 0));
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("small input fits the pipe");
    // Stdin stays open with no further traffic: the peer never
    // drains, never hangs up, and nothing else is enqueued.
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("stalled write fails within the drain wait");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(200),
        "must wait out the drain grace, not fail instantly"
    );
    drop(stdin);
    child.kill().ok();
    let mut out = Vec::new();
    let mut err = Vec::new();
    use std::io::Read as _;
    stdout.read_to_end(&mut out).expect("stdout reads");
    stderr.read_to_end(&mut err).expect("stderr reads");
    let stderr = String::from_utf8_lossy(&err);
    assert!(
        stderr.contains("drain_timeout"),
        "stalled write must be typed, got: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out);
    let first = stdout.lines().next().expect("hello completed");
    let hello: serde_json::Value =
        serde_json::from_str(first).expect("hello parses");
    assert_eq!(hello["method"], "adapter.handshakeHello");
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

/// Fixture with a 2MiB scripted snapshot on the bound target: one
/// look answer overflows the OS pipe buffer when nobody reads,
/// parking the writer mid-frame while the run loop stays responsive.
fn big_snapshot_script() -> String {
    let big = "x".repeat(2 * 1024 * 1024);
    format!(
        "[[snapshots]]\ntarget = \"seat-1\"\n\n[[snapshots.entries]]\nkind = \"text\"\nseq = 1\ntext = \"{big}\"\n"
    )
}

/// One small unscripted submit (~300B in, ~500B of result+persist
/// out): admitted under any cap, answered Submitted with no choices.
fn small_submit(id: &str, correlation: &str) -> Vec<u8> {
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": correlation,
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": "aGk=",
            }],
        },
    });
    let mut bytes = serde_json::to_vec(&frame).expect("serializes");
    bytes.push(b'\n');
    bytes
}

fn spawn_big(script: &std::path::Path, extra: &[&str]) -> std::process::Child {
    let mut args = vec![
        "--target",
        "seat-1",
        "--script",
        script.to_str().expect("utf8"),
        "--max-frame-bytes",
        "4194304",
    ];
    args.extend_from_slice(extra);
    Command::new(binary())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns")
}

/// A data write to a non-draining stdout blocks only up to the
/// negotiated drain wait, then fails with a typed backpressure
/// failure instead of queueing unboundedly.
#[test]
fn blocked_stdout_fails_within_the_drain_wait() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &[]);
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 250, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 0));
    // Seventeen small submits (each a ~500B result+persist pair) pile
    // behind the parked 2MiB frame and fill the 16-slot queue; the
    // last enqueue exceeds the 250ms drain wait and terminalizes the
    // session.
    for n in 11..=27 {
        wire.extend_from_slice(&small_submit(
            &n.to_string(),
            &format!("c-{n}"),
        ));
    }
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("small input fits the pipe");
    // Stdin stays open: the peer never drains, never hangs up.
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("adapter exits within the bound");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(200),
        "must wait out the drain grace, not fail instantly"
    );
    child.kill().ok();
    let output = child.wait_with_output().expect("adapter exits");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("drain_timeout"),
        "blocked write must be typed, got: {stderr}"
    );
}

/// The drain grace is the negotiated value, not the default: a 400ms
/// negotiated wait holds past the 250ms default before failing.
#[test]
fn blocked_stdout_fails_at_the_negotiated_drain_wait() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &[]);
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 400, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 0));
    for n in 11..=27 {
        wire.extend_from_slice(&small_submit(
            &n.to_string(),
            &format!("c-{n}"),
        ));
    }
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("small input fits the pipe");
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("adapter exits within the bound");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(350),
        "must honor the negotiated 400ms wait, not the 250ms default"
    );
    child.kill().ok();
}

/// Fence processing stays runnable while a data write is blocked:
/// the fence takes effect locally within the bound even though the
/// transport never drains.
#[test]
fn fence_takes_effect_while_write_is_blocked() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &[]);
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 250, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 0));
    let fence = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "fence-hint",
        },
    });
    let mut fence_bytes = serde_json::to_vec(&fence).expect("serializes");
    fence_bytes.push(b'\n');
    wire.extend_from_slice(&fence_bytes);
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("small input fits the pipe");
    drop(stdin);
    // Hold stdout unread: the pipe fills and parks the writer, so
    // the fence must take effect with the transport never draining.
    let stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let status = child.wait().expect("adapter exits");
    assert_eq!(status.code(), Some(0));
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "fence must act locally without waiting for the wire"
    );
    let mut err = Vec::new();
    use std::io::Read as _;
    stderr.read_to_end(&mut err).expect("stderr reads");
    drop(stdout);
    let stderr = String::from_utf8_lossy(&err);
    assert!(
        stderr.contains("fence_hint"),
        "fence must terminalize locally, got: {stderr}"
    );
}

/// A partially-emitted frame that cannot complete within the bound
/// makes the transport unusable and terminalizes locally: framing
/// integrity holds on the wire (every complete frame parses) and no
/// control bytes are injected mid-frame.
#[test]
fn partial_frame_terminalizes_with_framing_intact() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &[]);
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 250, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 0));
    let fence = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "fence-hint",
        },
    });
    let mut fence_bytes = serde_json::to_vec(&fence).expect("serializes");
    fence_bytes.push(b'\n');
    wire.extend_from_slice(&fence_bytes);
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("small input fits the pipe");
    drop(stdin);
    // Hold stdout unread: the pipe fills and parks the writer
    // mid-frame, so framing integrity is observed with the transport
    // never draining.
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let status = child.wait().expect("adapter exits");
    assert_eq!(status.code(), Some(0));
    let mut out = Vec::new();
    let mut err = Vec::new();
    use std::io::Read as _;
    stdout.read_to_end(&mut out).expect("stdout reads");
    stderr.read_to_end(&mut err).expect("stderr reads");
    let stdout = String::from_utf8_lossy(&out);
    // The stalled 2MiB answer is a partial tail with no trailing
    // newline: every newline-terminated chunk parses, and exactly one
    // complete frame (the hello request) precedes the cut. Only the
    // final segment may fail to parse, and only because it is cut.
    assert!(!out.ends_with(b"\n"), "the stalled frame is cut mid-write");
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.len() >= 2, "hello plus the cut tail, got {lines:?}");
    let (complete, tail) = lines.split_at(lines.len() - 1);
    assert_eq!(complete.len(), 1, "only the hello request completed");
    let frame: serde_json::Value =
        serde_json::from_str(complete[0]).expect("hello parses");
    assert_eq!(frame["method"], "adapter.handshakeHello");
    assert!(
        tail[0].starts_with("{\"id\""),
        "the tail is a cut frame prefix, got: {:?}",
        &tail[0][..tail[0].len().min(60)]
    );
    let stderr = String::from_utf8_lossy(&err);
    assert!(
        stderr.contains("fence_hint"),
        "stall terminalizes locally, got: {stderr}"
    );
}

/// A control reply is never injected into a partially written data
/// frame: a ping arriving after the stall gets no wire answer.
#[test]
fn no_control_reply_after_stall() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &[]);
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 250, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 0));
    let ping = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "11",
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    });
    let mut ping_bytes = serde_json::to_vec(&ping).expect("serializes");
    ping_bytes.push(b'\n');
    wire.extend_from_slice(&ping_bytes);
    let fence = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "fence-hint",
        },
    });
    let mut fence_bytes = serde_json::to_vec(&fence).expect("serializes");
    fence_bytes.push(b'\n');
    wire.extend_from_slice(&fence_bytes);
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("small input fits the pipe");
    drop(stdin);
    // Hold stdout unread so the stall is real: the ping reply stays
    // queued behind the parked frame and never reaches the wire.
    let mut stdout = child.stdout.take().expect("stdout piped");
    let status = child.wait().expect("adapter exits");
    assert_eq!(status.code(), Some(0));
    let mut out = Vec::new();
    use std::io::Read as _;
    stdout.read_to_end(&mut out).expect("stdout reads");
    drop(stdout);
    let stdout = String::from_utf8_lossy(&out);
    // The ping reply stays queued behind the parked 2MiB frame: the
    // wire ends mid-frame with no trailing newline, and the only
    // complete frame is the hello request.
    assert!(!out.ends_with(b"\n"), "the stalled frame is cut mid-write");
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.len() >= 2, "hello plus the cut tail");
    let (complete, tail) = lines.split_at(lines.len() - 1);
    assert_eq!(complete.len(), 1, "no ping reply after the stall");
    let frame: serde_json::Value =
        serde_json::from_str(complete[0]).expect("hello parses");
    assert_eq!(frame["method"], "adapter.handshakeHello");
    assert!(
        tail[0].starts_with("{\"id\""),
        "the tail is a cut frame prefix, got: {:?}",
        &tail[0][..tail[0].len().min(60)]
    );
}

/// Incoming traffic must not starve the stalled-writer watchdog:
/// binding-valid fence-hints every 5ms (which enqueue no output)
/// keep arriving while one 2MiB look answer is parked, yet the
/// session still terminalizes within the negotiated drain wait.
#[test]
fn incoming_traffic_cannot_starve_stalled_writer_watchdog() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &[]);
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 250, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 0));
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("small input fits the pipe");
    // Stdout stays unread (the stall); stderr drains on a thread so
    // fence diagnostics never back up.
    let stderr = child.stderr.take().expect("stderr piped");
    let drain = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut err = Vec::new();
        let _ = std::io::BufReader::new(stderr).read_to_end(&mut err);
        err
    });
    let stdout = child.stdout.take().expect("stdout piped");
    let fence = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "fence-hint",
        },
    });
    let mut fence_bytes = serde_json::to_vec(&fence).expect("serializes");
    fence_bytes.push(b'\n');
    // Fence every 5ms with stdin held open: continuous inbound
    // progress that enqueues nothing and proves nothing about the
    // writer.
    let code = loop {
        if let Some(status) = child.try_wait().expect("wait works") {
            break status.code();
        }
        if start.elapsed() > Duration::from_secs(10) {
            break None;
        }
        let _ = stdin.write_all(&fence_bytes);
        std::thread::sleep(Duration::from_millis(5));
    }
    .expect("stalled writer still terminalizes under inbound traffic");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(200),
        "must wait out the drain grace, not fail instantly"
    );
    drop(stdin);
    drop(stdout);
    child.kill().ok();
    let err = drain.join().expect("stderr drains");
    let stderr = String::from_utf8_lossy(&err);
    assert!(
        stderr.contains("drain_timeout"),
        "stalled write must be typed, got: {stderr}"
    );
}

/// A prime-wait expiring is not liveness expiry: with a 2s scripted
/// delay, 100ms prime, and 5000ms liveness, the look answers stale
/// (priming, not fence-terminalized) at ~100ms and the session stays
/// alive — a later ping and submit both answer, and the run ends
/// clean with no liveness timeout.
#[test]
fn prime_expiry_does_not_terminalize_liveness() {
    let script = "[[snapshots]]\ntarget = \"seat-1\"\ndelay_ms = 2000\n\n[[snapshots.entries]]\nkind = \"text\"\nseq = 1\ntext = \"slow hello\"\n";
    let (_dir, path) = fixture(script);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            path.to_str().expect("utf8"),
            "--max-frame-bytes",
            "4194304",
            "--liveness-timeout-ms",
            "5000",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns");
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 250, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 100));
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("hello+look land");
    // Hold stdin open with no further traffic: the prime wait must
    // resolve the look stale without ending the session.
    std::thread::sleep(Duration::from_millis(400));
    let ping = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "11",
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    });
    let mut ping_bytes = serde_json::to_vec(&ping).expect("serializes");
    ping_bytes.push(b'\n');
    stdin.write_all(&ping_bytes).expect("ping lands");
    let submit = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "12",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 0,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-live",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": "aGk=",
            }],
        },
    });
    let mut submit_bytes = serde_json::to_vec(&submit).expect("serializes");
    submit_bytes.push(b'\n');
    stdin.write_all(&submit_bytes).expect("submit lands");
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
    let mut term_bytes = serde_json::to_vec(&terminate).expect("serializes");
    term_bytes.push(b'\n');
    stdin.write_all(&term_bytes).expect("terminate lands");
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "session must end well before any liveness expiry"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let frames: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("frame parses"))
        .collect();
    let look = frames
        .iter()
        .find(|frame| frame["result"]["correlation_id"] == "look-10")
        .expect("look answered");
    assert_eq!(look["result"]["freshness"], "stale");
    assert_eq!(look["result"]["stale_reason_code"], "priming");
    assert_eq!(look["result"]["discontinuity"], false);
    assert!(
        frames
            .iter()
            .any(|frame| frame["result"] == serde_json::json!({})),
        "ping answered after the prime wait: {frames:?}"
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame["result"]["outcome"] == "Submitted"),
        "submit answered after the prime wait: {frames:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("liveness_timeout"),
        "no liveness expiry on a live binding: {stderr}"
    );
}

/// A clean EOF with bytes still parked on a stalled wire records
/// the drain failure without changing the session outcome: the 2MiB
/// answer parks on a full stdout pipe nobody drains, stdin closes,
/// and shutdown waits only the negotiated 250ms drain wait before
/// recording a typed drain_timeout and exiting with the session's
/// clean code. A fixed two-second wait with no diagnostic would fail
/// both the bound and the record below.
#[test]
fn clean_eof_on_stalled_drain_records_timeout_within_negotiated_wait() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &[]);
    let start = Instant::now();
    let mut wire = serde_json::to_vec(&hello_negotiated(
        4194304, 250, 16_777_216, 16_777_216,
    ))
    .expect("serializes");
    wire.push(b'\n');
    wire.extend_from_slice(&look_poll("10", "seat-1", 0));
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("small input fits the pipe");
    // EOF with the 2MiB answer still parked on the wire.
    drop(stdin);
    // Stdout stays unread (the stall); the run must still
    // terminalize within the negotiated bound.
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("stalled drain still terminalizes");
    assert_eq!(code, 0);
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "shutdown must be bound by the negotiated 250ms wait, not a fixed delay"
    );
    let output = child.wait_with_output().expect("adapter exits");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("drain_timeout"),
        "stalled shutdown drain must be typed, got: {stderr}"
    );
}

/// A transport that dies during the handshake fails the handshake
/// now: with stdout closed up front, the hello-request write fails
/// and the handshake observes the writer-dead event instead of
/// parking until the handshake timeout. Exit is refusal either way;
/// only the bound distinguishes, and it must be far short of the
/// 8s timeout.
#[test]
fn handshake_observes_writer_death_without_waiting_for_timeout() {
    let (_dir, script) = fixture(&big_snapshot_script());
    let mut child = spawn_big(&script, &["--handshake-timeout-ms", "8000"]);
    let start = Instant::now();
    // Close stdout before any traffic: the first write fails.
    drop(child.stdout.take().expect("stdout piped"));
    // Stdin stays open and silent: nothing inbound will ever wake a
    // parked handshake, so only writer observation can fail it fast.
    let code = poll_exit(&mut child, start, Duration::from_secs(20))
        .expect("dead transport fails the handshake");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "writer death must fail the handshake promptly, not at the timeout"
    );
    child.kill().ok();
    let output = child.wait_with_output().expect("adapter exits");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("transport_dead")
            || stderr.contains("transport_unusable"),
        "dead transport must be typed, got: {stderr}"
    );
}
