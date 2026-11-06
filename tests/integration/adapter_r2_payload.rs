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

fn sink_codes(sink: &[DiagnosticEntry]) -> Vec<&str> {
    sink.iter().map(|entry| entry.code.as_str()).collect()
}
/// HELLO SHAPE (adapter-wire-schema): a resume snapshot that is not
/// base64 refuses the binding. Ignoring resume contents does not make
/// invalid encoding well-formed: only the hello request goes out,
/// and the run ends in refusal.
#[test]
fn malformed_snapshot_b64_refuses_binding() {
    let config = test_config("");
    let mut hello = hello_result();
    hello["result"]["snapshot_b64"] = json!("%%%not-base64");
    let input = transcript(&[hello]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_REFUSAL);
    let frames = split_frames(&output);
    // The hello request is enqueued before the refusal is known, but
    // the writer thread may not drain it before the run loop fails
    // the handshake on preloaded input — a live supervisor cannot
    // send the result without having received the request, so this
    // race is test-only. Either way nothing beyond hello goes out.
    assert!(
        frames.len() <= 1,
        "at most the hello request goes out: {frames:?}"
    );
    if let Some(first) = frames.first() {
        assert_eq!(first["method"], "adapter.handshakeHello");
    }
}

/// HELLO SHAPE (adapter-wire-schema): a base64 resume snapshot is
/// accepted (and ignored) — the binding proceeds.
#[test]
fn valid_snapshot_b64_accepted() {
    let config = test_config("");
    let mut hello = hello_result();
    hello["result"]["snapshot_b64"] = json!("aGk=");
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
    let input = transcript(&[hello, terminate]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert_eq!(split_frames(&output).len(), 1);
}

/// GENERIC PROFILE (adapter-wire-schema): an ACP-only snapshot kind
/// under the generic profile is rejected before any wire traffic —
/// a trusted fixture is not permission to emit a non-conforming
/// frame.
#[test]
fn generic_profile_rejects_acp_snapshot_kind() {
    let script = r#"
[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "agent"
seq = 1
text = "hello"
"#;
    let config = test_config(script);
    let input = transcript(&[hello_result()]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_REFUSAL);
    assert!(output.is_empty(), "no frames before refusal");
    assert!(
        sink.iter()
            .any(|entry| entry.code == "invalid_script_for_profile"),
        "rejection is diagnosed: {:?}",
        sink_codes(&sink)
    );
}

/// ACP PROFILE (adapter-wire-schema): the agent-surface kinds are
/// admitted alongside text, and a scripted agent entry answers
/// fresh with its kind preserved.
#[test]
fn acp_profile_accepts_agent_snapshot_kind() {
    let script = r#"
[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "agent"
seq = 7
text = "agent hello"
"#;
    let mut config = test_config(script);
    config.profile = "acp".to_string();
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
    let input = transcript(&[hello_result(), look, terminate]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames.len(), 2, "hello, then the look answer");
    assert_eq!(frames[1]["result"]["freshness"], "fresh");
    assert_eq!(frames[1]["result"]["entries"][0]["kind"], "agent");
    assert_eq!(frames[1]["result"]["entries"][0]["seq"], 7);
}

/// LIFECYCLE DOMAIN (adapter-wire-schema): every closed kind —
/// ready, degraded, failed, progress, heartbeat — emits; the
/// domain is profile-independent.
#[test]
fn all_lifecycle_kinds_emit() {
    let script = r#"
[[events]]
kind = "ready"
code = "adapter_ready"

[[events]]
kind = "degraded"
code = "link_degraded"

[[events]]
kind = "failed"
code = "agent_failed"

[[events]]
kind = "progress"
code = "turn_progress"

[[events]]
kind = "heartbeat"
code = "still_alive"
"#;
    let config = test_config(script);
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
    let input = transcript(&[hello_result(), terminate]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames.len(), 6, "hello plus five lifecycle events");
    let kinds: Vec<&str> = frames[1..]
        .iter()
        .map(|frame| frame["params"]["kind"].as_str().unwrap_or("?"))
        .collect();
    assert_eq!(
        kinds,
        vec!["ready", "degraded", "failed", "progress", "heartbeat"]
    );
}

/// LIFECYCLE DOMAIN (adapter-wire-schema): an unknown event kind is
/// rejected at startup, before any frame goes out — the outbound
/// counterpart of the closed supervision-signal domain.
#[test]
fn unknown_lifecycle_kind_refused_at_startup() {
    let script = r#"
[[events]]
kind = "not-a-kind"
code = "whatever"
"#;
    let config = test_config(script);
    let input = transcript(&[hello_result()]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_REFUSAL);
    assert!(output.is_empty(), "no frames before refusal");
    assert!(
        sink.iter()
            .any(|entry| entry.code == "invalid_script_for_profile"),
        "rejection is diagnosed: {:?}",
        sink_codes(&sink)
    );
}
