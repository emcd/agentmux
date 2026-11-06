//! Run_stdio transport-saturation evidence: gated-writer and
//! parked-reader harnesses proving the dispatch boundary stays live
//! under queued-output saturation — control admission, fence
//! processing, and bounded drain failure with deferred drops. Each
//! integration file owns its fixture helpers outright.
//!
use std::collections::HashMap;
use std::io::BufReader;
use std::sync::{Arc, Mutex};

use agentmux::adapter::choice::DiagnosticEntry;
use agentmux::adapter::handshake::{LimitsProposal, VersionOffer};
use agentmux::adapter::lifecycle::ManualClock;
use agentmux::adapter::{AdapterConfig, EXIT_CLEAN, EXIT_REFUSAL, run_stdio};
use serde_json::{Value, json};

fn test_config(script_toml: &str) -> AdapterConfig {
    let script = agentmux::adapter::script::parse_script(script_toml)
        .expect("script parses");
    let mut latency = HashMap::from([("default".to_string(), 5_000_u64)]);
    latency.insert("text".to_string(), 5_000);
    AdapterConfig {
        executable: "agentmux-adapter".to_string(),
        profile: "generic".to_string(),
        target_session: "seat-1".to_string(),
        handshake_timeout_ms: 30_000,
        liveness_timeout_ms: 60_000,
        offer: VersionOffer {
            major: 2,
            min_minor: 0,
            max_minor: 1,
        },
        proposal: LimitsProposal {
            max_frame_bytes: 65_536,
            queue_tx_bytes: 16_777_216,
            queue_rx_bytes: 16_777_216,
            max_control_pending: 32,
            max_events_per_second: 100,
            window_seconds: 1,
            max_submits_in_flight: 64,
            max_choices_in_flight: 16,
            max_looks_in_flight: 8,
            drain_wait_ms: 250,
            latency_declarations: latency,
        },
        script,
    }
}

fn limits_value() -> Value {
    json!({
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
    })
}

fn hello_result() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "1",
        "result": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "adapter_generation": 3,
            "selected_major": 2,
            "selected_minor": 1,
            "snapshot_b64": "",
            "negotiated_limits": limits_value(),
        },
    })
}

fn transcript(frames: &[Value]) -> Vec<u8> {
    let mut wire = Vec::new();
    for frame in frames {
        let bytes = serde_json::to_vec(frame).expect("frame serializes");
        wire.extend_from_slice(&bytes);
        wire.push(b'\n');
    }
    wire
}

fn sink_codes(sink: &[DiagnosticEntry]) -> Vec<&str> {
    sink.iter().map(|entry| entry.code.as_str()).collect()
}

/// Gated transport for saturation tests: write() parks until the
/// test opens the gate, then appends bytes in order. Lets the test
/// fill both lanes while the writer is stalled, so admission and
/// terminalization under queued-output saturation — not timing — is
/// under test.
#[derive(Clone)]
struct GateWriter {
    gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
    out: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for GateWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let (lock, cvar) = &*self.gate;
        let mut open = lock.lock().expect("gate lock");
        while !*open {
            open = cvar.wait(open).expect("gate wait");
        }
        self.out.lock().expect("out lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Gate handle plus captured output and diagnostics from a gated run.
type GatedRun = (
    i32,
    Arc<(Mutex<bool>, std::sync::Condvar)>,
    Arc<Mutex<Vec<u8>>>,
    Vec<DiagnosticEntry>,
);

fn run_gated_transcript(config: &AdapterConfig, input: &[u8]) -> GatedRun {
    let clock = ManualClock::new(1_000);
    let reader = BufReader::new(std::io::Cursor::new(input.to_vec()));
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let out = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = GateWriter {
        gate: gate.clone(),
        out: out.clone(),
    };
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    let code = run_stdio(config, &clock, reader, writer, &mut sink);
    (code, gate, out, sink)
}

fn open_gate_and_collect(
    gate: &Arc<(Mutex<bool>, std::sync::Condvar)>,
    out: &Arc<Mutex<Vec<u8>>>,
    frames: usize,
) -> Vec<Value> {
    let (lock, cvar) = &**gate;
    *lock.lock().expect("gate lock") = true;
    cvar.notify_all();
    let start = std::time::Instant::now();
    loop {
        let wire = out.lock().expect("out lock").clone();
        let count = wire.iter().filter(|byte| **byte == b'\n').count();
        if count >= frames {
            let text = String::from_utf8(wire).expect("frames are UTF-8");
            return text
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_str(line).expect("frame parses"))
                .collect();
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "writer must drain after the gate opens"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Control is admitted under data saturation: with the writer
/// parked on the handshake request and sixteen data answers filling
/// the sixteen-deep data channel to the brim, a ping reply still
/// enqueues on its own lane — no control refusal, no escalation —
/// and the session ends clean at EOF. A shared FIFO would block the
/// control enqueue behind the full data queue and terminalize with
/// refusal instead.
#[test]
fn control_admitted_under_data_saturation() {
    let script = r#"
[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "x"
"#;
    let config = test_config(script);
    let mut frames = vec![hello_result()];
    for n in 20..=35 {
        frames.push(json!({
            "jsonrpc": "2.0",
            "id": n.to_string(),
            "method": "relay.look",
            "params": {
                "protocol_major": 2,
                "protocol_minor": 1,
                "runtime_instance": "rt-9",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "correlation_id": format!("look-{n}"),
                "look_handle_id": format!("h-{n}"),
                "prime_timeout_ms": 0,
            },
        }));
    }
    frames.push(json!({
        "jsonrpc": "2.0",
        "id": "40",
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    }));
    let input = transcript(&frames);
    let (code, gate, out, sink) = run_gated_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink)
            .iter()
            .any(|entry| entry.starts_with("control_")),
        "no control-lane refusal under saturation: {:?}",
        sink_codes(&sink)
    );
    let drained = open_gate_and_collect(&gate, &out, 18);
    assert_eq!(drained.len(), 18, "hello, 16 looks, ping: {drained:?}");
    assert_eq!(drained[0]["method"], "adapter.handshakeHello");
    assert!(
        drained
            .iter()
            .any(|frame| frame.get("result") == Some(&json!({}))),
        "ping reply was queued and drains: {drained:?}"
    );
}

/// A fence-hint is processed with the data lane saturated: sixteen
/// queued answers (behind the parked handshake write) do not park
/// supervision traffic, the fence
/// terminalizes locally, and EOF ends the run clean — bounded, never
/// hanging on the stalled writer.
#[test]
fn fence_processed_under_data_saturation() {
    let script = r#"
[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "x"
"#;
    let config = test_config(script);
    let mut frames = vec![hello_result()];
    for n in 20..=35 {
        frames.push(json!({
            "jsonrpc": "2.0",
            "id": n.to_string(),
            "method": "relay.look",
            "params": {
                "protocol_major": 2,
                "protocol_minor": 1,
                "runtime_instance": "rt-9",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "correlation_id": format!("look-{n}"),
                "look_handle_id": format!("h-{n}"),
                "prime_timeout_ms": 0,
            },
        }));
    }
    frames.push(json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "fence-hint",
        },
    }));
    let input = transcript(&frames);
    let (code, gate, out, sink) = run_gated_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "fence_hint"),
        "fence takes effect under saturation: {:?}",
        sink_codes(&sink)
    );
    let drained = open_gate_and_collect(&gate, &out, 17);
    assert_eq!(drained.len(), 17, "hello plus 16 looks: {drained:?}");
}

/// Reader that delivers the transcript and then parks instead of
/// signalling EOF, so the run loop keeps slicing on its 50ms waits
/// while the writer is stalled. Lets the test observe the drain
/// watchdog firing with deferred work outstanding.
struct ParkAfterReader {
    inner: std::io::Cursor<Vec<u8>>,
    done: Arc<std::sync::atomic::AtomicBool>,
}

impl std::io::Read for ParkAfterReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let n = self.inner.read(buf)?;
            if n > 0 || self.done.load(std::sync::atomic::Ordering::Acquire) {
                return Ok(n);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// A data lane that overflows defers instead of parking the run
/// loop, and control stays serviceable throughout: with the writer
/// parked and sixteen data frames saturating the channel, a
/// seventeenth data answer is deferred (not retried in place), the
/// ping is still dispatched and queued on its own lane, and only the
/// drain watchdog — after its full bounded wait with zero completed
/// writes — terminalizes with refusal. The deferred frame never
/// reaches the wire (dropped at shutdown with a typed record) while
/// the ping reply does once the gate opens.
#[test]
fn overflowing_data_defers_and_control_stays_serviceable() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let script = r#"
[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "x"
"#;
    let config = test_config(script);
    let mut frames = vec![hello_result()];
    for n in 20..=36 {
        frames.push(json!({
            "jsonrpc": "2.0",
            "id": n.to_string(),
            "method": "relay.look",
            "params": {
                "protocol_major": 2,
                "protocol_minor": 1,
                "runtime_instance": "rt-9",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "correlation_id": format!("look-{n}"),
                "look_handle_id": format!("h-{n}"),
                "prime_timeout_ms": 0,
            },
        }));
    }
    frames.push(json!({
        "jsonrpc": "2.0",
        "id": "40",
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    }));
    let wire = transcript(&frames);
    let clock = ManualClock::new(1_000);
    let done = Arc::new(AtomicBool::new(false));
    let reader = BufReader::new(ParkAfterReader {
        inner: std::io::Cursor::new(wire),
        done: done.clone(),
    });
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let out = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = GateWriter {
        gate: gate.clone(),
        out: out.clone(),
    };
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    let code = run_stdio(&config, &clock, reader, writer, &mut sink);
    done.store(true, Ordering::Release);
    // The drain watchdog fires after its full bounded wait with zero
    // completed writes: the stall is real, and the session refuses
    // rather than hanging.
    assert_eq!(code, EXIT_REFUSAL);
    assert!(
        sink_codes(&sink).contains(&"drain_timeout"),
        "bounded stall must be typed: {:?}",
        sink_codes(&sink)
    );
    assert!(
        sink_codes(&sink).contains(&"deferred_unwritten"),
        "the overflowed frame is dropped at shutdown with a record: {:?}",
        sink_codes(&sink)
    );
    // The ping was dispatched and queued WHILE the data lane was
    // full: control never waited for the wire-dependent enqueue.
    let drained = open_gate_and_collect(&gate, &out, 18);
    assert_eq!(drained.len(), 18, "hello, 16 looks, ping: {drained:?}");
    assert!(
        drained
            .iter()
            .any(|frame| frame.get("result") == Some(&json!({}))),
        "ping reply was served during the data stall: {drained:?}"
    );
}

/// A clean shutdown drains deferred frames on a responsive
/// transport instead of dropping them: twenty immediate looks
/// saturate the sixteen-deep data channel behind the parked
/// handshake write (four deferred), EOF ends the run, and the gate
/// opens 100ms later — well within the negotiated 250ms drain wait.
/// The shutdown keeps the writer connected and keeps flushing as
/// completions free capacity, so all twenty-one frames reach the
/// wire with a clean exit and neither a drain_timeout nor a
/// deferred_unwritten record. A single best-effort flush followed by
/// an immediate disconnect would drop the four deferred frames and
/// mistype the healthy drain as a stall.
#[test]
fn clean_shutdown_drains_deferred_on_responsive_transport() {
    let script = r#"
[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "x"
"#;
    let config = test_config(script);
    let mut frames = vec![hello_result()];
    for n in 20..=39 {
        frames.push(json!({
            "jsonrpc": "2.0",
            "id": n.to_string(),
            "method": "relay.look",
            "params": {
                "protocol_major": 2,
                "protocol_minor": 1,
                "runtime_instance": "rt-9",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "correlation_id": format!("look-{n}"),
                "look_handle_id": format!("h-{n}"),
                "prime_timeout_ms": 0,
            },
        }));
    }
    let input = transcript(&frames);
    let clock = ManualClock::new(1_000);
    let reader = BufReader::new(std::io::Cursor::new(input));
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let out = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = GateWriter {
        gate: gate.clone(),
        out: out.clone(),
    };
    // Open the gate 100ms in, while the shutdown drain is still
    // waiting: a responsive writer must be allowed to drain.
    let opener = gate;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let (lock, cvar) = &*opener;
        *lock.lock().expect("gate lock") = true;
        cvar.notify_all();
    });
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    let code = run_stdio(&config, &clock, reader, writer, &mut sink);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink).contains(&"drain_timeout"),
        "a draining transport is not a stall: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"deferred_unwritten"),
        "nothing dropped on a draining transport: {:?}",
        sink_codes(&sink)
    );
    let wire = out.lock().expect("writer lock").clone();
    let drained: Vec<Value> = String::from_utf8(wire)
        .expect("frames are UTF-8")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("frame parses"))
        .collect();
    assert_eq!(drained.len(), 21, "hello plus twenty answers: {drained:?}");
    assert_eq!(drained[0]["method"], "adapter.handshakeHello");
}

/// Slow-but-steady transport for continuous-busy evidence: every
/// write takes a fixed few milliseconds, so a rapid burst of
/// dispatches outruns the writer and exercises the deferred queue,
/// per-turn flush, and notify coalescing under sustained load —
/// without ever stalling.
struct SlowWriter {
    out: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for SlowWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        std::thread::sleep(std::time::Duration::from_millis(3));
        self.out.lock().expect("out lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Continuous busy work completes without loss or stall: twenty
/// looks plus a ping against a 3ms writer keep the data lane
/// saturated and the deferred queue active while control stays
/// serviceable throughout. A dropped (rather than coalesced)
/// wakeup would stall the drain and terminalize with a timeout;
/// an unbounded wakeup queue would grow without bound alongside
/// the bounded frame backlog. Neither happens: exit is clean, all
/// twenty-two frames land, and the ping reply is among them.
#[test]
fn continuous_busy_work_completes_without_loss_or_stall() {
    let script = r#"
[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "x"
"#;
    let config = test_config(script);
    let mut frames = vec![hello_result()];
    for n in 20..=39 {
        frames.push(json!({
            "jsonrpc": "2.0",
            "id": n.to_string(),
            "method": "relay.look",
            "params": {
                "protocol_major": 2,
                "protocol_minor": 1,
                "runtime_instance": "rt-9",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "correlation_id": format!("look-{n}"),
                "look_handle_id": format!("h-{n}"),
                "prime_timeout_ms": 0,
            },
        }));
    }
    frames.push(json!({
        "jsonrpc": "2.0",
        "id": "40",
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    }));
    let input = transcript(&frames);
    let clock = ManualClock::new(1_000);
    let reader = BufReader::new(std::io::Cursor::new(input));
    let out = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = SlowWriter { out: out.clone() };
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    let code = run_stdio(&config, &clock, reader, writer, &mut sink);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink).contains(&"drain_timeout"),
        "steady progress is not a stall: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"deferred_unwritten"),
        "nothing dropped on a draining transport: {:?}",
        sink_codes(&sink)
    );
    let wire = out.lock().expect("out lock").clone();
    let drained: Vec<Value> = String::from_utf8(wire)
        .expect("frames are UTF-8")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("frame parses"))
        .collect();
    assert_eq!(drained.len(), 22, "hello, 20 looks, ping: {drained:?}");
    assert_eq!(drained[0]["method"], "adapter.handshakeHello");
    assert!(
        drained
            .iter()
            .any(|frame| frame.get("result") == Some(&json!({}))),
        "ping reply served under sustained load: {drained:?}"
    );
}
