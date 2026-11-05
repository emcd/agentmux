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

fn terminate() -> Value {
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

fn fence_hint() -> Value {
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

const CHOICE_SCRIPT: &str = r#"
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

const QUICK_ACK_SCRIPT: &str = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;

fn valid_chosen_reply() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    })
}

fn valid_cancelled_reply() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"cancelled": {"reason_code": "user_cancelled"}},
        },
    })
}

fn sink_codes(sink: &[DiagnosticEntry]) -> Vec<&str> {
    sink.iter().map(|entry| entry.code.as_str()).collect()
}

fn count_code(sink: &[DiagnosticEntry], code: &str) -> usize {
    sink.iter().filter(|entry| entry.code == code).count()
}

fn submit_results(frames: &[Value]) -> Vec<&Value> {
    frames
        .iter()
        .filter(|frame| {
            frame
                .get("result")
                .is_some_and(|r| r.get("outcome").is_some())
        })
        .collect()
}

/// CHOICE-OUTCOME-EXACTLY-ONE-KEY (choice): a result whose outcome
/// carries BOTH chosen and cancelled is malformed and keeps the call
/// open; only a later valid reply terminalizes the turn.
#[test]
fn both_keys_malformed_kept_open() {
    let config = test_config(CHOICE_SCRIPT);
    let both = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {
                "chosen": {"option_id": "yes", "decided_by": "op-9"},
                "cancelled": {"reason_code": "user_cancelled"},
            },
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        both,
        valid_chosen_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_result"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
    assert_eq!(submits[0]["result"]["correlation_id"], "c-2");
}

/// CHOICE-OUTCOME-EXACTLY-ONE-KEY (choice): an empty outcome object
/// is malformed and keeps the call open; a later valid cancelled
/// reply completes the turn with the scripted ack.
#[test]
fn neither_key_malformed_kept_open() {
    let config = test_config(CHOICE_SCRIPT);
    let empty = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {"correlation_id": "ch-2", "outcome": {}},
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        empty,
        valid_cancelled_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_result"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
    assert_eq!(submits[0]["result"]["correlation_id"], "c-2");
}

/// CHOICE-OUTCOME-EXACTLY-ONE-KEY (choice): an outcome with an extra
/// key beside chosen is malformed and keeps the call open; a later
/// valid reply resolves the turn.
#[test]
fn extra_key_malformed_kept_open() {
    let config = test_config(CHOICE_SCRIPT);
    let extra = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {
                "chosen": {"option_id": "yes", "decided_by": "op-9"},
                "extra": {},
            },
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        extra,
        valid_chosen_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_result"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
}

/// CHOICE-OUTCOME-FIELD-TYPES (choice): a cancelled outcome with a
/// non-string reason_code is malformed and keeps the call open; a
/// later valid cancelled reply completes the turn.
#[test]
fn mistyped_cancelled_fields_kept_open() {
    let config = test_config(CHOICE_SCRIPT);
    let mistyped = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"cancelled": {"reason_code": 123}},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        mistyped,
        valid_cancelled_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_result"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
}

/// CHOICE-ERROR-EXEMPTION (choice): a standard transport error with
/// no correlation echo is admitted by table match and terminalizes
/// the turn; no echo-mismatch diagnostic fires.
#[test]
fn standard_error_without_echo_admitted() {
    let config = test_config(CHOICE_SCRIPT);
    let standard_error = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {"code": -32603, "message": "boom"},
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        standard_error,
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
    assert!(
        !sink_codes(&sink).contains(&"correlation_echo_mismatch"),
        "sink: {:?}",
        sink_codes(&sink)
    );
}

/// CHOICE-ERROR-EXEMPTION (choice): a -32000 contract error echoing
/// the open correlation is admitted and terminalizes the turn.
#[test]
fn contract_error_matching_echo_admitted() {
    let config = test_config(CHOICE_SCRIPT);
    let contract_error = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "relay refused",
            "data": {
                "code": "relay_error",
                "reason": "x",
                "correlation_id": "ch-2",
            },
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        contract_error,
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
    let _ = sink;
}

/// CHOICE-ECHO-MATCH (choice): a result echoing the wrong correlation
/// is kept open; only the correctly echoed reply resolves the turn.
#[test]
fn mismatched_echo_kept_open() {
    let config = test_config(CHOICE_SCRIPT);
    let wrong_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-WRONG",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        wrong_echo,
        valid_chosen_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"correlation_echo_mismatch"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
    assert_eq!(submits[0]["result"]["correlation_id"], "c-2");
}

/// CHOICE-ERROR-EXEMPTION (choice): correlation_invalid WITH an echo
/// is admitted by exemption and terminalizes the turn.
#[test]
fn correlation_invalid_with_echo_admitted() {
    let config = test_config(CHOICE_SCRIPT);
    let invalid_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "bad correlation",
            "data": {
                "code": "correlation_invalid",
                "reason": "r",
                "correlation_id": "ch-2",
            },
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        invalid_echo,
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
    let _ = sink;
}

/// CHOICE-ERROR-EXEMPTION (choice): correlation_invalid WITHOUT an
/// echo is admitted by exemption and terminalizes the turn.
#[test]
fn correlation_invalid_without_echo_admitted() {
    let config = test_config(CHOICE_SCRIPT);
    let invalid_no_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "bad correlation",
            "data": {"code": "correlation_invalid", "reason": "r"},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        invalid_no_echo,
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
    let _ = sink;
}

/// CHOICE-ERROR-SHAPE (choice): a -32000 error whose data lacks the
/// required reason is malformed (no exemption for ill-formed errors)
/// and keeps the call open; a later valid reply resolves the turn.
#[test]
fn malformed_correlation_invalid_kept_open() {
    let config = test_config(CHOICE_SCRIPT);
    let malformed_error = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "bad correlation",
            "data": {"code": "correlation_invalid"},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        malformed_error,
        valid_chosen_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_error"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
}

/// CHOICE-ERROR-SHAPE (choice): an error with a non-numeric code is
/// malformed and keeps the call open; a later valid reply resolves.
#[test]
fn nonnumeric_error_code_kept_open() {
    let config = test_config(CHOICE_SCRIPT);
    let string_code = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {"code": "-32000", "message": "boom"},
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        string_code,
        valid_chosen_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_error"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
}

/// CHOICE-ERROR-SHAPE (choice): a -32000 error whose data lacks the
/// required reason is malformed and keeps the call open; a later
/// valid reply resolves the turn.
#[test]
fn missing_reason_kept_open() {
    let config = test_config(CHOICE_SCRIPT);
    let no_reason = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {
            "code": -32000,
            "message": "relay refused",
            "data": {"code": "relay_error"},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        no_reason,
        valid_chosen_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"malformed_reply_error"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
}

/// CHOICE-REPLY-EXCLUSIVITY (choice): a reply carrying BOTH result
/// and error, or NEITHER, is discarded with the call kept open; a
/// later valid reply resolves the turn in each sub-case.
#[test]
fn both_and_neither_result_error_refused() {
    // Sub-case A: reply carrying both result and error.
    let config = test_config(CHOICE_SCRIPT);
    let both = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
        "error": {"code": -32603, "message": "boom"},
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        both,
        valid_chosen_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"result_error_exclusive"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");

    // Sub-case B: reply carrying neither result nor error (id only).
    let config = test_config(CHOICE_SCRIPT);
    let neither = json!({"jsonrpc": "2.0", "id": "2"});
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-2"),
        neither,
        valid_chosen_reply(),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"result_error_exclusive"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
}

/// HANDSHAKE-BOOTSTRAP-ENVELOPE (handshake): a hello reply without
/// the jsonrpc version marker is refused.
#[test]
fn hello_without_version_refused() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let mut hello = hello_result();
    hello
        .as_object_mut()
        .expect("hello is an object")
        .remove("jsonrpc");
    let input = transcript(&[hello, terminate()]);
    let (code, _, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_REFUSAL);
}

/// HANDSHAKE-BOOTSTRAP-ENVELOPE (handshake): a hello reply carrying
/// both result and error is refused.
#[test]
fn hello_both_result_and_error_refused() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let mut hello = hello_result();
    hello.as_object_mut().expect("hello is an object").insert(
        "error".to_string(),
        json!({"code": -32603, "message": "boom"}),
    );
    let input = transcript(&[hello, terminate()]);
    let (code, _, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_REFUSAL);
}

fn persist_reply(persisted: Value, extra: Option<(&str, Value)>) -> Value {
    let mut result = json!({
        "correlation_id": "persist-2",
        "persisted": persisted,
    });
    if let Some((key, value)) = extra {
        result
            .as_object_mut()
            .expect("result is an object")
            .insert(key.to_string(), value);
    }
    json!({"jsonrpc": "2.0", "id": "2", "result": result})
}

/// PERSIST-OUTCOME-COMMIT (persist): a persisted:true reply retires
/// the write; teardown records no unknown.
#[test]
fn persist_true_retires() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        persist_reply(json!(true), None),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert_eq!(submits[0]["result"]["outcome"], "Submitted");
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "sink: {:?}",
        sink_codes(&sink)
    );
}

/// PERSIST-OUTCOME-REFUSED (persist): persisted:false with a
/// contract refusal code retires refused; teardown records no
/// unknown.
#[test]
fn persist_refused_with_code_retires() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        persist_reply(
            json!(false),
            Some(("refusal_code", json!("retention-full"))),
        ),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
    assert!(
        sink_codes(&sink).contains(&"persist_refused"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "sink: {:?}",
        sink_codes(&sink)
    );
}

/// PERSIST-OUTCOME-SHAPE (persist): persisted:false with a refusal
/// code outside the contract set is malformed and keeps the write
/// open; a later valid reply retires it cleanly.
#[test]
fn invalid_refusal_code_malformed() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        persist_reply(
            json!(false),
            Some(("refusal_code", json!("bogus-code"))),
        ),
        persist_reply(json!(true), None),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"persist_outcome_malformed"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
}

/// PERSIST-OUTCOME-SHAPE (persist): persisted:true with a refusal
/// code present is malformed and keeps the write open; a later valid
/// reply retires it cleanly.
#[test]
fn true_with_refusal_present_malformed() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        persist_reply(
            json!(true),
            Some(("refusal_code", json!("retention-full"))),
        ),
        persist_reply(json!(true), None),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"persist_outcome_malformed"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
}

/// PERSIST-OUTCOME-SHAPE (persist): persisted:false with a
/// non-string refusal code is malformed and keeps the write open; a
/// later valid reply retires it cleanly.
#[test]
fn wrongtype_refusal_malformed() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        persist_reply(json!(false), Some(("refusal_code", json!(7)))),
        persist_reply(json!(true), None),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"persist_outcome_malformed"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
}

/// PERSIST-OUTCOME-SHAPE (persist): a reply missing the persisted
/// field is malformed and keeps the write open; a later valid
/// refused reply resolves as refused with no unknown and no
/// unknown-reply diagnostic.
#[test]
fn malformed_then_valid_resolves_as_valid() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let missing = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {"correlation_id": "persist-2"},
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        missing,
        persist_reply(
            json!(false),
            Some(("refusal_code", json!("retention-full"))),
        ),
        terminate(),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        sink_codes(&sink).contains(&"persist_refused"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"persist_unknown"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    assert!(
        !sink_codes(&sink).contains(&"unknown_reply"),
        "sink: {:?}",
        sink_codes(&sink)
    );
    let submits = submit_results(&frames);
    assert_eq!(submits.len(), 1, "frames: {frames:?}");
}

/// PERSIST-TRACKER-SURVIVES-MALFORMED (persist): a malformed persist
/// reply leaves the tracker entry intact, so a later fence records
/// exactly one unknown for the still-open write.
#[test]
fn malformed_leaves_tracker_intact() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let missing = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {"correlation_id": "persist-2"},
    });
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        missing,
        fence_hint(),
        terminate(),
    ]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert_eq!(
        count_code(&sink, "persist_unknown"),
        1,
        "sink: {:?}",
        sink_codes(&sink)
    );
}

/// PERSIST-TEARDOWN-DRAIN (persist): an open persist at fence time
/// records unknown; the run still exits clean.
#[test]
fn fence_drains_persist() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let input = transcript(&[
        hello_result(),
        submit_request("10", "c-1"),
        fence_hint(),
        terminate(),
    ]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink_codes(&sink).contains(&"persist_unknown"),
        "sink: {:?}",
        sink_codes(&sink)
    );
}

/// PERSIST-TEARDOWN-DRAIN (persist): an open persist at EOF records
/// unknown; the run still exits clean.
#[test]
fn eof_drains_persist() {
    let config = test_config(QUICK_ACK_SCRIPT);
    let input = transcript(&[hello_result(), submit_request("10", "c-1")]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink_codes(&sink).contains(&"persist_unknown"),
        "sink: {:?}",
        sink_codes(&sink)
    );
}
