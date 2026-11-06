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

/// Custom config helper: starts from the pinned harness defaults and
/// applies one proposal mutation, so the proposal under test is explicit
/// at the call site while the rest of the harness stays identical.
fn test_config_with(
    script_toml: &str,
    mutate: impl FnOnce(&mut LimitsProposal),
) -> AdapterConfig {
    let mut config = test_config(script_toml);
    mutate(&mut config.proposal);
    config
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

/// Custom hello limits helper: renders the negotiated limits from the
/// config's own proposal so each custom test negotiates exactly what it
/// proposes (the contract requires negotiated <= proposal per field).
fn limits_value_for(proposal: &LimitsProposal) -> Value {
    json!({
        "max_frame_bytes": proposal.max_frame_bytes,
        "queue_tx_bytes": proposal.queue_tx_bytes,
        "queue_rx_bytes": proposal.queue_rx_bytes,
        "max_control_pending": proposal.max_control_pending,
        "max_events_per_second": proposal.max_events_per_second,
        "window_seconds": proposal.window_seconds,
        "max_submits_in_flight": proposal.max_submits_in_flight,
        "max_choices_in_flight": proposal.max_choices_in_flight,
        "max_looks_in_flight": proposal.max_looks_in_flight,
        "drain_wait_ms": proposal.drain_wait_ms,
    })
}

fn hello_result() -> Value {
    hello_result_with_limits(limits_value())
}

fn hello_result_with_limits(limits: Value) -> Value {
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
            "negotiated_limits": limits,
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

fn terminate_signal() -> Value {
    json!({
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
    })
}

fn fence_hint_signal() -> Value {
    json!({
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

/// Writer that parks every frame: with no completions, queue
/// accounting never releases, so cap-exhaustion tests are
/// deterministic rather than racing the writer thread.
struct BlockingWriter;

impl std::io::Write for BlockingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        std::thread::sleep(std::time::Duration::from_secs(3600));
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn run_transcript_blocked(
    config: &AdapterConfig,
    input: &[u8],
) -> (i32, Vec<DiagnosticEntry>) {
    let clock = ManualClock::new(1_000);
    let reader = BufReader::new(std::io::Cursor::new(input.to_vec()));
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    let code = run_stdio(config, &clock, reader, BlockingWriter, &mut sink);
    (code, sink)
}

fn result_for(frames: &[Value], correlation: &str) -> Value {
    frames
        .iter()
        .find(|frame| {
            frame.get("result").and_then(|result| {
                result.get("correlation_id").and_then(Value::as_str)
            }) == Some(correlation)
        })
        .unwrap_or_else(|| panic!("no result for {correlation}: {frames:?}"))
        .clone()
}

fn sink_codes(sink: &[DiagnosticEntry]) -> Vec<&str> {
    sink.iter().map(|entry| entry.code.as_str()).collect()
}

/// Requirement: Adapter submit replies preserve the submission-evidence trichotomy (adapter-submit-acks).
#[test]
fn proven_submitted_never_downgraded() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config(script);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        fence_hint_signal(),
        terminate_signal(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    // hello, submit result, persist marker; fence-hint and terminate
    // emit no frames once the turn already completed.
    assert_eq!(frames.len(), 3, "unexpected frames: {frames:?}");
    let submitted = result_for(&frames, "c-1");
    assert_eq!(submitted["result"]["outcome"], "Submitted");
    assert!(
        submitted["result"].get("evidence_basis").is_none(),
        "proven submission carries no evidence_basis: {submitted:?}"
    );
    assert!(
        !sink_codes(&sink).contains(&"submission_unknown"),
        "completed Submitted turn records no unknown: {:?}",
        sink_codes(&sink)
    );
}

/// Requirement: Provable non-submission is a no-effect outcome that releases the single-flight gate (adapter-single-flight).
#[test]
fn provable_nonsubmission_is_noeffect_and_releases_gate() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "transport_unavailable"
"#;
    let config = test_config(script);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        submit_request("11", "c-2"),
        terminate_signal(),
    ]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    // hello, c-1 result, c-1 persist, c-2 result, c-2 persist.
    assert_eq!(frames.len(), 5, "unexpected frames: {frames:?}");
    let first = result_for(&frames, "c-1");
    assert_eq!(first["result"]["outcome"], "TransportUnavailable");
    assert_eq!(first["result"]["evidence_basis"], "provable_non_submission");
    // The gate released: unscripted c-2 takes the default Submitted
    // path instead of RefusedBusy.
    let second = result_for(&frames, "c-2");
    assert_eq!(second["result"]["outcome"], "Submitted");
}

/// Requirement: Unproven writes resolve unknown and hold the single-flight gate until observed cessation (adapter-single-flight).
#[test]
fn absent_proof_resolves_unknown_and_holds_gate() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submission_unknown"
"#;
    let config = test_config(script);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        submit_request("11", "c-2"),
        terminate_signal(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    // hello, c-1 unknown result, c-1 persist marker, c-2 refused busy.
    // The persist marker still emits: only agent-effect bytes are
    // withheld on unknown, never the adapter-owned resume marker.
    assert_eq!(frames.len(), 4, "unexpected frames: {frames:?}");
    let first = result_for(&frames, "c-1");
    assert_eq!(first["result"]["outcome"], "SubmissionUnknown");
    assert_eq!(first["result"]["evidence_basis"], "unproven_write");
    // The gate stays held: c-2 is refused busy, never queued.
    let second = result_for(&frames, "c-2");
    assert_eq!(second["result"]["outcome"], "RefusedBusy");
    // The unknown outcome came from the terminal itself and the gate
    // stayed held, yet the sink records no submission_unknown: the turn
    // completed (no pending choice at teardown), so there is nothing to
    // interrupt. Teardown only records submission_unknown for a held
    // turn with a pending submit.
    assert!(
        !sink_codes(&sink).contains(&"submission_unknown"),
        "completed unknown turn records no unknown: {:?}",
        sink_codes(&sink)
    );
}

/// Requirement: Authoritative control-lane admission failure terminalizes instead of dropping (adapter-control-lane).
#[test]
fn control_bound_exhaustion_terminalizes() {
    let script = r#"
[[events]]
kind = "ready"
code = "adapter_ready"

[[events]]
kind = "degraded"
code = "link_degraded"
"#;
    let config = test_config_with(script, |proposal| {
        proposal.max_control_pending = 1;
    });
    let limits = limits_value_for(&config.proposal);
    let input =
        transcript(&[hello_result_with_limits(limits), terminate_signal()]);
    // The writer parks the first event, so no completion releases the
    // lane: the second authoritative event cannot be admitted and
    // terminalizes instead of dropping.
    let (code, sink) = run_transcript_blocked(&config, &input);
    // The second authoritative event cannot be admitted: it
    // terminalizes instead of dropping.
    assert_eq!(code, EXIT_REFUSAL);
    assert!(
        sink_codes(&sink).contains(&"control_lane_full"),
        "control refusal must be diagnosed: {:?}",
        sink_codes(&sink)
    );
}

/// Requirement: The adapter-TX cap bounds the transmit outbox under matched-pair negotiation (adapter-outbox-bounds).
#[test]
fn direction_caps_matched_pairs() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config_with(script, |proposal| {
        proposal.queue_tx_bytes = 1_000_000;
        proposal.queue_rx_bytes = 1_000_000;
    });
    let mut limits = limits_value_for(&config.proposal);
    limits["queue_tx_bytes"] = json!(200);
    let input = transcript(&[
        hello_result_with_limits(limits),
        submit_request("10", "c-1"),
        terminate_signal(),
    ]);
    let (code, _, sink) = run_transcript(&config, &input);
    // The ~200B submit result fits the 200B TX cap but the persist
    // marker overflows it: adapter-TX bounds the outbox, not RX.
    assert_eq!(code, EXIT_REFUSAL);
    assert!(
        sink_codes(&sink).contains(&"outbox_full"),
        "TX cap refusal must be diagnosed: {:?}",
        sink_codes(&sink)
    );
}

/// Requirement: Inbound acquisition is bounded by the lesser of the frame cap and the adapter-RX cap (adapter-queue-caps).
#[test]
fn rx_cap_bounds_inbound_reads() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config(script);
    let mut limits = limits_value_for(&config.proposal);
    limits["queue_rx_bytes"] = json!(100);
    let input = transcript(&[
        hello_result_with_limits(limits),
        submit_request("10", "c-1"),
        terminate_signal(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    // The ~300B submit frame exceeds min(frame cap, RX cap) = 100B, so
    // it is discarded at acquisition with a typed framing failure and
    // the session continues: the terminate signal ends it clean with
    // no submit terminal fabricated for the refused frame.
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink_codes(&sink).contains(&"oversize_frame"),
        "discard must record the framing failure: {:?}",
        sink_codes(&sink)
    );
    let text = String::from_utf8(output).expect("stdout is UTF-8");
    assert!(
        !text.contains("\"outcome\":\"Submitted\""),
        "no terminal for the discarded frame: {text}"
    );
}

/// Requirement: Authoritative lifecycle events past the negotiated rate terminalize as event floods (adapter-lifecycle-events).
#[test]
fn event_rate_enforced_with_window() {
    let script = r#"
[[events]]
kind = "ready"
code = "first_ready"

[[events]]
kind = "ready"
code = "second_ready"

[[events]]
kind = "ready"
code = "third_ready"
"#;
    let config = test_config_with(script, |proposal| {
        proposal.max_events_per_second = 2;
        proposal.window_seconds = 60;
    });
    let limits = limits_value_for(&config.proposal);
    let input =
        transcript(&[hello_result_with_limits(limits), terminate_signal()]);
    let (code, _, sink) = run_transcript(&config, &input);
    // The third ready inside the 60s window exceeds the rate of 2/s:
    // authoritative events terminalize instead of coalescing.
    assert_eq!(code, EXIT_REFUSAL);
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "event_flood"),
        "flood must terminalize as event_flood: {:?}",
        sink_codes(&sink)
    );
}
