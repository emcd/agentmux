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

/// Exempt errors admit by table match regardless of echo: a standard
/// transport error carrying a NON-matching echo still resolves the
/// open choice (an exemption removes the requirement; it never adds
/// a matching criterion).
#[test]
fn standard_error_nonmatching_echo_admitted_choice() {
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
    let wrong_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32603,
            "message": "boom",
            "data": {"correlation_id": "ch-WRONG"},
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
    // The wrong echo is admitted, not kept open: no second reply is
    // needed to terminalize the turn.
    let input =
        transcript(&[hello_result(), submit_request("10", "c-2"), wrong_echo]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink).contains(&"correlation_echo_mismatch"),
        "exempt echo must not mismatch: {:?}",
        sink_codes(&sink)
    );
    let _ = valid;
    let frames = split_frames(&output);
    assert_eq!(frames[2]["result"]["outcome"], "Submitted");
}

/// Exempt errors admit by table match regardless of echo: a
/// correlation_invalid carrying a NON-matching echo still resolves
/// the open choice by table match instead.
#[test]
fn correlation_invalid_nonmatching_echo_admitted_choice() {
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
    let wrong_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "bogus correlation",
            "data": {
                "code": "correlation_invalid",
                "reason": "rejected",
                "correlation_id": "ch-WRONG",
            },
        },
    });
    let input =
        transcript(&[hello_result(), submit_request("10", "c-2"), wrong_echo]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink).contains(&"correlation_echo_mismatch"),
        "exempt echo must not mismatch: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    assert_eq!(frames[2]["result"]["outcome"], "Submitted");
}

/// Exempt errors admit on persist writes too: a standard error with
/// no echo ends the open write without commit proof — retired, never
/// persisted, never left hanging for a second terminal.
#[test]
fn standard_error_no_echo_admitted_persist() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config(script);
    let err = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {"code": -32603, "message": "boom"},
    });
    let input =
        transcript(&[hello_result(), submit_request("10", "c-1"), err]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink_codes(&sink).contains(&"persist_error"),
        "admitted error ends the write: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "no hanging write left behind: {:?}",
        sink_codes(&sink)
    );
}

/// Exempt errors admit on persist writes with any echo: a standard
/// error carrying a non-matching echo still ends the open write.
#[test]
fn standard_error_nonmatching_echo_admitted_persist() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config(script);
    let err = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32600,
            "message": "bad request",
            "data": {"correlation_id": "persist-2X"},
        },
    });
    let input =
        transcript(&[hello_result(), submit_request("10", "c-1"), err]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink).contains(&"correlation_echo_mismatch"),
        "exempt echo must not mismatch: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "no hanging write left behind: {:?}",
        sink_codes(&sink)
    );
}

/// A correlation_invalid error ends the open persist by table match
/// with or without an echo.
#[test]
fn correlation_invalid_no_echo_admitted_persist() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config(script);
    let err = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "bogus correlation",
            "data": {"code": "correlation_invalid", "reason": "rejected"},
        },
    });
    let input =
        transcript(&[hello_result(), submit_request("10", "c-1"), err]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "no hanging write left behind: {:?}",
        sink_codes(&sink)
    );
}

/// A correlation_invalid error with a non-matching echo still ends
/// the open persist by table match.
#[test]
fn correlation_invalid_nonmatching_echo_admitted_persist() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config(script);
    let err = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "bogus correlation",
            "data": {
                "code": "correlation_invalid",
                "reason": "rejected",
                "correlation_id": "persist-2X",
            },
        },
    });
    let input =
        transcript(&[hello_result(), submit_request("10", "c-1"), err]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        !sink_codes(&sink).contains(&"correlation_echo_mismatch"),
        "exempt echo must not mismatch: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "no hanging write left behind: {:?}",
        sink_codes(&sink)
    );
}

/// A standard error without its required message is malformed: the
/// JSON-RPC error object requires message text, and its absence
/// keeps the call open without resetting liveness.
#[test]
fn standard_error_missing_message_kept_open() {
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
    let no_message = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {"code": -32603},
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
        no_message,
        valid,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter()
            .any(|entry| entry.code == "malformed_reply_error"),
        "missing message is malformed: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
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
}

/// A -32000 contract error whose optional details field is present
/// with the wrong type is malformed: optional means absence allowed,
/// not wrong present type. The call stays open for a valid reply.
#[test]
fn contract_error_bad_details_kept_open() {
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
    let bad_details = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "refused",
            "data": {
                "code": "relay_error",
                "reason": "busy",
                "correlation_id": "ch-2",
                "details": "not-an-object",
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
        bad_details,
        valid,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter()
            .any(|entry| entry.code == "malformed_reply_error"),
        "mistyped details are malformed: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
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
}

/// An exempt error with a present but non-string echo is malformed:
/// the exemption waives the echo requirement, never the type of a
/// field the peer actually sent. The call stays open for a valid
/// reply and liveness does not reset.
#[test]
fn correlation_invalid_nonstring_echo_kept_open() {
    let script = r#"
[clock]
advances_ms = [30000, 60000]

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
    let submit = json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-2",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "text",
                "payload_b64": "aGk=",
            }],
        },
    });
    // correlation_invalid, but the echoed correlation is a number.
    let bad_echo = json!({
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
    let valid = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    // Valid submit at 31000 sets the deadline to 91000; the
    // mistyped echo at 91000 must not reset it.
    let input = transcript(&[hello_result(), submit, bad_echo, valid]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter()
            .any(|entry| entry.code == "malformed_reply_error"),
        "present non-string echo is malformed: {:?}",
        sink_codes(&sink)
    );
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "liveness_timeout"),
        "expiry, not EOF drain: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    let terminals: Vec<&serde_json::Value> = frames
        .iter()
        .filter(|frame| {
            frame
                .get("result")
                .is_some_and(|r| r.get("outcome").is_some())
        })
        .collect();
    assert_eq!(terminals.len(), 0, "no terminal from a malformed echo");
}

/// The same present-type rule applies on persist calls: a
/// correlation_invalid error with a numeric echo keeps the write
/// open instead of resolving it.
#[test]
fn correlation_invalid_nonstring_echo_kept_open_persist() {
    let script = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;
    let config = test_config(script);
    let bad_echo = json!({
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
    let valid = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {"correlation_id": "persist-2", "persisted": true},
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        bad_echo,
        valid,
    ]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter()
            .any(|entry| entry.code == "malformed_reply_error"),
        "present non-string echo is malformed: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "valid reply resolved the write: {:?}",
        sink_codes(&sink)
    );
}
