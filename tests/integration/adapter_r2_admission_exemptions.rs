//! Originated-reply exemption and shape evidence: the echo
//! exemption applies only to JSON-RPC standard transport errors and
//! to well-shaped -32000 correlation_invalid rejections; every
//! other error needs its echo. Unknown minor fields inside choice
//! outcomes are permitted; unknown-method notifications get no
//! reply. Each integration file owns its fixture helpers outright.
//!
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

fn sink_codes(sink: &[DiagnosticEntry]) -> Vec<&str> {
    sink.iter().map(|entry| entry.code.as_str()).collect()
}

/// Unknown minor fields INSIDE chosen are permitted: the exactly-one
/// rule constrains the outer outcome discriminator, not the chosen
/// payload. A chosen outcome carrying a future minor field still
/// resolves the choice.
#[test]
fn chosen_with_unknown_minor_field_admitted() {
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
    let config = test_config(script);
    let future_chosen = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {
                "option_id": "yes",
                "decided_by": "op-9",
                "future_minor_field": "forward-compat",
            }},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        future_chosen,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let terminals: Vec<&Value> = frames
        .iter()
        .filter(|frame| {
            frame
                .get("result")
                .is_some_and(|r| r.get("outcome").is_some())
        })
        .collect();
    assert_eq!(terminals.len(), 1, "choice resolves once: {frames:?}");
    assert_eq!(terminals[0]["result"]["outcome"], "Submitted");
    assert!(
        !sink_codes(&sink).contains(&"malformed_reply_result"),
        "unknown minor fields inside chosen are permitted: {:?}",
        sink_codes(&sink)
    );
}

/// A non-standard numeric error code carries no exemption: code 42
/// without an echo is a missing-required-echo malformation and the
/// call stays open for the later valid choice result.
#[test]
fn nonstandard_error_code_requires_echo() {
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
    let config = test_config(script);
    let code_42_no_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {"code": 42, "message": "something bespoke"},
    });
    let valid = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        code_42_no_echo,
        valid,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_error"),
        "code 42 without echo is malformed, not exempt: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    let terminals: Vec<&Value> = frames
        .iter()
        .filter(|frame| {
            frame
                .get("result")
                .is_some_and(|r| r.get("outcome").is_some())
        })
        .collect();
    assert_eq!(
        terminals.len(),
        1,
        "call kept open, then resolved: {frames:?}"
    );
    assert_eq!(terminals[0]["result"]["outcome"], "Submitted");
}

/// An unknown-method NOTIFICATION gets no reply at all: with no id
/// there is no error frame, not even id:null. The session continues
/// to a clean terminate.
#[test]
fn unknown_method_notification_gets_no_reply() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config(script);
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "relay.nope",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
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
    let input = transcript(&[hello_result(), notification, terminate]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink_codes(&sink).contains(&"unknown_method_notification"),
        "notification discard is recorded: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    assert!(
        frames.iter().all(|frame| frame.get("error").is_none()),
        "no error reply to a notification: {frames:?}"
    );
    assert!(
        frames
            .iter()
            .all(|frame| frame.get("id").is_none_or(|id| !id.is_null())),
        "no id:null frame on the wire: {frames:?}"
    );
}

/// A nonstandard numeric code cannot borrow the contract exemption
/// by CLAIMING it: {code:42, data:{code:"correlation_invalid"}}
/// without an echo is still a missing-required-echo malformation —
/// the exemption needs the actual -32000 contract-error shape — and
/// the call stays open for the later valid choice.
#[test]
fn nonstandard_code_cannot_claim_correlation_invalid() {
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
    let config = test_config(script);
    let claimed = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": 42,
            "message": "bespoke",
            "data": {
                "code": "correlation_invalid",
                "reason": "not a -32000 contract error",
            },
        },
    });
    let valid = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        claimed,
        valid,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_error"),
        "claimed exemption without -32000 shape is malformed: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    let terminals: Vec<&Value> = frames
        .iter()
        .filter(|frame| {
            frame
                .get("result")
                .is_some_and(|r| r.get("outcome").is_some())
        })
        .collect();
    assert_eq!(
        terminals.len(),
        1,
        "call kept open, then resolved: {frames:?}"
    );
    assert_eq!(terminals[0]["result"]["outcome"], "Submitted");
}

/// A -32000 correlation_invalid error without its required reason
/// is malformed (the contract-error shape demands code AND reason),
/// not exempt: the call stays open for the later valid reply.
#[test]
fn correlation_invalid_without_reason_is_malformed() {
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
    let config = test_config(script);
    let reasonless = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "contract failure",
            "data": {"code": "correlation_invalid"},
        },
    });
    let valid = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        reasonless,
        valid,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_error"),
        "reasonless contract error is malformed: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    let terminals: Vec<&Value> = frames
        .iter()
        .filter(|frame| {
            frame
                .get("result")
                .is_some_and(|r| r.get("outcome").is_some())
        })
        .collect();
    assert_eq!(
        terminals.len(),
        1,
        "call kept open, then resolved: {frames:?}"
    );
    assert_eq!(terminals[0]["result"]["outcome"], "Submitted");
}

/// Unknown minor fields INSIDE cancelled are permitted like in
/// chosen: a cancelled outcome carrying a future minor field still
/// resolves (here as a turn cancellation, which the scripted turn
/// completes as unknown-outcome handling dictates).
#[test]
fn cancelled_with_unknown_minor_field_admitted() {
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
    let config = test_config(script);
    let cancelled = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"cancelled": {
                "reason_code": "relay.cancelled/operator",
                "decided_by": "op-9",
                "future_minor_field": "forward-compat",
            }},
        },
    });
    let input =
        transcript(&[hello_result(), submit_request("10", "c-2"), cancelled]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink).contains(&"malformed_reply_result"),
        "unknown minor fields inside cancelled are permitted: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    assert!(
        frames.iter().any(|frame| frame
            .get("result")
            .is_some_and(|r| { r.get("outcome").is_some() })),
        "cancelled outcome resolves the turn: {frames:?}"
    );
}
