//! Transport-scheduling evidence for the reference adapter: the
//! same mixed-lane input trace replayed repeatedly must keep
//! identical per-lane wire order and identical terminal outcomes on
//! every run, even though the writer thread races the run loop on
//! every enqueue. Harness mirrors the other R2/stdio modules: each
//! integration file owns its fixture helpers outright.
//!
//! The mechanism under evidence is structural, not timing: a single
//! enqueuer stamps a total order, each lane channel is FIFO, an
//! in-progress frame always finishes before the next boundary, and
//! the priority rule only ever chooses between already-queued heads.
//! Cross-lane interleave may vary with scheduling; same-lane order
//! and terminals cannot.

use std::collections::HashMap;
use std::io::BufReader;
use std::sync::{Arc, Mutex};

use agentmux::adapter::choice::DiagnosticEntry;
use agentmux::adapter::handshake::{LimitsProposal, VersionOffer};
use agentmux::adapter::lifecycle::ManualClock;
use agentmux::adapter::{AdapterConfig, EXIT_CLEAN, run_stdio};
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

fn submit_request(id: &str, correlation: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": correlation,
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "text",
                "payload_b64": "aGk=",
            }],
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

fn run_transcript(
    config: &AdapterConfig,
    input: &[u8],
) -> (i32, Vec<u8>, Vec<DiagnosticEntry>) {
    let clock = ManualClock::new(1_000);
    let reader = BufReader::new(std::io::Cursor::new(input.to_vec()));
    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = SharedWriter(shared.clone());
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    let code = run_stdio(config, &clock, reader, writer, &mut sink);
    let output = shared.lock().expect("writer lock").clone();
    (code, output, sink)
}

/// Shared in-process transport for `run_stdio`: the writer thread
/// owns this handle and appends frames behind the mutex, so the
/// harness observes the complete transcript after shutdown drains.
#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("writer lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn split_frames(wire: &[u8]) -> Vec<Value> {
    let text = str::from_utf8(wire).expect("frames are UTF-8");
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("frame parses"))
        .collect()
}

const SCRIPT: &str = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"

[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "hello"

[[events]]
kind = "ready"
code = "adapter_ready"
"#;

/// Scheduling-independence evidence: the same mixed-lane input
/// trace replayed twenty-five times keeps identical per-lane wire
/// order and identical terminal outcomes on every run, even though
/// the writer thread races the run loop on every enqueue. The
/// mechanism is structural, not timing: a single enqueuer stamps a
/// total order, each lane channel is FIFO, an in-progress frame
/// always finishes before the next boundary, and the priority rule
/// only ever chooses between already-queued heads. Cross-lane
/// interleave may vary with scheduling; same-lane order and
/// terminals cannot.
#[test]
fn mixed_lane_replay_keeps_per_lane_order_across_runs() {
    let config = test_config(SCRIPT);
    let terminate = json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "terminate",
        },
    });
    let look = json!({
        "jsonrpc": "2.0",
        "id": "11",
        "method": "relay.look",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "look-1",
            "look_handle_id": "h-1",
            "prime_timeout_ms": 0,
        },
    });
    let ping = json!({
        "jsonrpc": "2.0",
        "id": "12",
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        look,
        ping,
        terminate,
    ]);
    fn lane_of(frame: &Value) -> &'static str {
        if frame["method"] == "adapter.lifecycleEvent"
            || frame.get("result").is_some_and(|result| {
                result.as_object().is_some_and(|object| object.is_empty())
            })
        {
            "control"
        } else {
            "data"
        }
    }
    let mut baseline_data: Vec<Value> = Vec::new();
    let mut baseline_control: Vec<Value> = Vec::new();
    for run in 0..25 {
        let (code, output, _) = run_transcript(&config, &input);
        assert_eq!(code, EXIT_CLEAN, "run {run} exits clean");
        let frames = split_frames(&output);
        assert_eq!(frames.len(), 6, "run {run}: {frames:?}");
        let mut data = Vec::new();
        let mut control = Vec::new();
        for frame in &frames {
            match lane_of(frame) {
                "control" => control.push(frame.clone()),
                _ => data.push(frame.clone()),
            }
        }
        assert!(
            data.iter()
                .any(|frame| frame.get("result").is_some_and(|r| r
                    .get("outcome")
                    == Some(&json!("Submitted")))),
            "run {run} submits: {frames:?}"
        );
        if run == 0 {
            baseline_data = data;
            baseline_control = control;
        } else {
            assert_eq!(
                data, baseline_data,
                "run {run} data lane identical: {frames:?}"
            );
            assert_eq!(
                control, baseline_control,
                "run {run} control lane identical: {frames:?}"
            );
        }
    }
}

/// Byte-identical replay through the lockstep seam: the same
/// mixed-lane dispatch trace (hello, submit, look, ping, persist
/// marker, terminate) run three times through `run_replay_stdio`
/// with a deliberately slow writer yields byte-identical
/// transcripts in exact enqueue order. The seam is structural, not
/// statistical: every admitted enqueue completes before the next
/// begins, so the wire order is exactly the enqueue order under
/// every thread schedule — writer selection and completion dispatch
/// are controlled by the quiesce barrier rather than observed
/// afterward. A slow writer is used on purpose: without the
/// barrier, all six frames would queue before the first write
/// finished and priority scheduling would emit control first
/// (hello, ready, submitted, ping, persist, look). Live scheduling
/// may interleave differently under contention; that is outside the
/// replay guarantee by the frozen contract's own scope (identical
/// traces, not external IO scheduling).
#[test]
fn mixed_lane_replay_is_byte_identical() {
    struct SlowWriter {
        out: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for SlowWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            // Slow enough that an unquiesced run loop would enqueue
            // the whole transcript before the first write lands.
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.out
                .lock()
                .expect("writer lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"

[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "hello"

[[events]]
kind = "ready"
code = "adapter_ready"
"#;
    let ping = json!({
        "jsonrpc": "2.0",
        "id": "12",
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    });
    let look = json!({
        "jsonrpc": "2.0",
        "id": "11",
        "method": "relay.look",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "look-1",
            "look_handle_id": "h-1",
            "prime_timeout_ms": 0,
        },
    });
    let terminate = json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": "terminate",
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        look,
        ping,
        terminate,
    ]);
    let mut transcripts = Vec::new();
    for run in 0..3 {
        let config = test_config(script);
        let clock = ManualClock::new(1_000);
        let reader = BufReader::new(std::io::Cursor::new(input.clone()));
        let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = SlowWriter {
            out: shared.clone(),
        };
        let mut sink: Vec<DiagnosticEntry> = Vec::new();
        let code = agentmux::adapter::run_replay_stdio(
            &config, &clock, reader, writer, &mut sink,
        );
        assert_eq!(code, EXIT_CLEAN, "run {run} exits clean");
        transcripts.push(shared.lock().expect("writer lock").clone());
    }
    assert_eq!(
        transcripts[0], transcripts[1],
        "identical trace replays byte-identical"
    );
    assert_eq!(
        transcripts[1], transcripts[2],
        "identical trace replays byte-identical"
    );
    // Exact enqueue order — not priority order. Without the quiesce
    // barrier the slow writer would emit hello, ready, submitted,
    // ping first (control lane first), and this assertion would fail.
    let kinds: Vec<String> = split_frames(&transcripts[0])
        .iter()
        .map(|frame| {
            if frame.get("method").is_some() {
                frame["method"].as_str().unwrap_or("?").to_string()
            } else if frame
                .get("result")
                .is_some_and(|r| r.get("outcome").is_some())
            {
                "result.outcome".to_string()
            } else if frame["result"]["freshness"] == "fresh" {
                "result.fresh".to_string()
            } else if frame.get("result").is_some_and(|r| {
                r.as_object().is_some_and(|object| object.is_empty())
            }) {
                "result.empty".to_string()
            } else {
                format!("{frame:?}")
            }
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "adapter.handshakeHello",
            "adapter.lifecycleEvent",
            "result.outcome",
            "adapter.persistState",
            "result.fresh",
            "result.empty",
        ],
        "lockstep replay keeps exact enqueue order: {kinds:?}"
    );
}
