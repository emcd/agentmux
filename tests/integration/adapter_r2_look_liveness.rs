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

fn submit_members(id: &str, correlation: &str, members: Value) -> Value {
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
            "members": members,
        },
    })
}

fn valid_member(message_id: &str) -> Value {
    json!({
        "message_id": message_id,
        "target_session": "seat-1",
        "adapter_generation": 3,
        "content_type": "text",
        "payload_b64": "aGk=",
    })
}

fn look_request(
    id: &str,
    correlation: &str,
    handle: Option<&str>,
    lines: Option<Value>,
    offset: Option<Value>,
    prime_timeout_ms: Value,
) -> Value {
    let mut params = serde_json::Map::from_iter([
        ("protocol_major".to_string(), json!(2)),
        ("protocol_minor".to_string(), json!(1)),
        ("runtime_instance".to_string(), json!("rt-9")),
        ("target_session".to_string(), json!("seat-1")),
        ("adapter_generation".to_string(), json!(3)),
        ("correlation_id".to_string(), json!(correlation)),
    ]);
    if let Some(handle) = handle {
        params.insert("look_handle_id".to_string(), json!(handle));
    }
    if let Some(lines) = lines {
        params.insert("lines".to_string(), lines);
    }
    if let Some(offset) = offset {
        params.insert("offset".to_string(), offset);
    }
    params.insert("prime_timeout_ms".to_string(), prime_timeout_ms);
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "relay.look",
        "params": Value::Object(params),
    })
}

fn ping(id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    })
}

fn signal(name: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "relay.supervisionSignal",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "signal": name,
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

fn has_liveness_timeout(sink: &[DiagnosticEntry]) -> bool {
    sink.iter().any(|entry| {
        entry.code == "liveness_timeout" || entry.detail == "liveness_timeout"
    })
}

fn terminalize_details(sink: &[DiagnosticEntry]) -> Vec<&str> {
    sink.iter()
        .filter(|entry| entry.code == "terminalize")
        .map(|entry| entry.detail.as_str())
        .collect()
}

fn find_by_id<'a>(frames: &'a [Value], id: &str) -> Option<&'a Value> {
    frames
        .iter()
        .find(|frame| frame.get("id").and_then(Value::as_str) == Some(id))
}

const QUICK_ACK: &str = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"

[[turns]]
correlation = "c-2"
ack = "submitted"
"#;

const SNAPSHOT: &str = r#"
[[snapshots]]
target = "seat-1"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "hello"
"#;

const SNAPSHOT_DELAYED: &str = r#"
[[snapshots]]
target = "seat-1"
delay_ms = 200

[[snapshots.entries]]
kind = "text"
seq = 1
text = "hello"
"#;

/// Requirement: Submit execution and ack trichotomy (every member validated).
///
/// A later member missing its message id is a structural refusal
/// echoing the request correlation; the gate is never held, so the
/// next well-formed submit proceeds Submitted.
#[test]
fn every_member_validated() {
    let config = test_config(QUICK_ACK);
    let bad = submit_members(
        "10",
        "c-1",
        json!([
            valid_member("m-1"),
            {
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "text",
                "payload_b64": "aGk=",
            },
        ]),
    );
    let good = submit_members("11", "c-2", json!([valid_member("m-1")]));
    let input = transcript(&[hello_result(), bad, good, signal("terminate")]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let refusal = find_by_id(&frames, "10").expect("refusal frame");
    assert_eq!(refusal["error"]["code"], -32000);
    assert_eq!(refusal["error"]["data"]["code"], "validation_structural");
    assert_eq!(refusal["error"]["data"]["correlation_id"], "c-1");
    let result = find_by_id(&frames, "11").expect("submit result");
    assert_eq!(result["result"]["outcome"], "Submitted");
    assert_eq!(result["result"]["correlation_id"], "c-2");
}

/// Requirement: Submit execution and ack trichotomy (payload present string, never defaulted).
///
/// A member without `payload_b64` and a member with a non-string
/// `payload_b64` are each structural refusals echoing c-1; neither
/// defaults to "".
#[test]
fn payload_missing_and_nonstring_refused() {
    for (tag, member) in [
        (
            "missing",
            json!({
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "text",
            }),
        ),
        (
            "nonstring",
            json!({
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "text",
                "payload_b64": 7,
            }),
        ),
    ] {
        let config = test_config(QUICK_ACK);
        let bad = submit_members("10", "c-1", json!([member]));
        let input = transcript(&[hello_result(), bad, signal("terminate")]);
        let (code, output, _) = run_transcript(&config, &input);
        assert_eq!(code, EXIT_CLEAN, "sub-case {tag}");
        let frames = split_frames(&output);
        let refusal = find_by_id(&frames, "10").expect("refusal frame");
        assert_eq!(refusal["error"]["code"], -32000, "sub-case {tag}");
        assert_eq!(
            refusal["error"]["data"]["code"], "validation_structural",
            "sub-case {tag}"
        );
        assert_eq!(
            refusal["error"]["data"]["correlation_id"], "c-1",
            "sub-case {tag}"
        );
        assert!(
            refusal["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("base64"),
            "refusal names the payload shape, sub-case {tag}: {refusal:?}"
        );
    }
}

/// Requirement: Submit execution and ack trichotomy (unknown content type refused).
///
/// A later member with an unknown content type is a structural
/// refusal naming the unknown type, never a default latency.
#[test]
fn later_member_unknown_content_type_refused() {
    let config = test_config(QUICK_ACK);
    let bad = submit_members(
        "10",
        "c-1",
        json!([
            valid_member("m-1"),
            {
                "message_id": "m-2",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "nope",
                "payload_b64": "aGk=",
            },
        ]),
    );
    let input = transcript(&[hello_result(), bad, signal("terminate")]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let refusal = find_by_id(&frames, "10").expect("refusal frame");
    assert_eq!(refusal["error"]["code"], -32000);
    assert_eq!(refusal["error"]["data"]["code"], "validation_structural");
    assert_eq!(refusal["error"]["data"]["correlation_id"], "c-1");
    assert!(
        refusal["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("nope"),
        "refusal names the unknown content type: {refusal:?}"
    );
}

/// Requirement: Submit execution and ack trichotomy (member binding refused).
///
/// A later member naming a foreign target is a structural refusal;
/// the gate is never held, so a well-formed retry proceeds.
#[test]
fn later_member_foreign_target_refused() {
    let config = test_config(QUICK_ACK);
    let bad = submit_members(
        "10",
        "c-1",
        json!([
            valid_member("m-1"),
            {
                "message_id": "m-2",
                "target_session": "elsewhere",
                "adapter_generation": 3,
                "content_type": "text",
                "payload_b64": "aGk=",
            },
        ]),
    );
    let good = submit_members("11", "c-2", json!([valid_member("m-1")]));
    let input = transcript(&[hello_result(), bad, good, signal("terminate")]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let refusal = find_by_id(&frames, "10").expect("refusal frame");
    assert_eq!(refusal["error"]["code"], -32000);
    assert_eq!(refusal["error"]["data"]["code"], "validation_structural");
    assert_eq!(refusal["error"]["data"]["correlation_id"], "c-1");
    let result = find_by_id(&frames, "11").expect("submit result");
    assert_eq!(result["result"]["outcome"], "Submitted");
    assert_eq!(result["result"]["correlation_id"], "c-2");
}

/// Requirement: Look poll responses (zero prime timeout means no wait).
///
/// With a 200ms-delayed snapshot, a zero prime timeout answers stale
/// priming immediately: exactly hello plus the look result, and the
/// terminate produces nothing further.
#[test]
fn zero_prime_means_no_wait() {
    let config = test_config(SNAPSHOT_DELAYED);
    let look = look_request("11", "look-1", Some("h-1"), None, None, json!(0));
    let input = transcript(&[hello_result(), look, signal("terminate")]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames.len(), 2, "unexpected frames: {frames:?}");
    assert_eq!(frames[1]["result"]["freshness"], "stale");
    assert_eq!(frames[1]["result"]["stale_reason_code"], "priming");
    assert_eq!(frames[1]["result"]["correlation_id"], "look-1");
}

/// Requirement: Look poll responses (look handle required).
///
/// A look without `look_handle_id` is a structural refusal echoing
/// look-1; the entry retires, so a retry with the handle works.
#[test]
fn missing_handle_refused() {
    let config = test_config(SNAPSHOT);
    let bad = look_request("11", "look-1", None, None, None, json!(0));
    let retry =
        look_request("12", "look-1", Some("h-1"), None, None, json!(0));
    let input = transcript(&[hello_result(), bad, retry, signal("terminate")]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames.len(), 3, "unexpected frames: {frames:?}");
    let refusal = find_by_id(&frames, "11").expect("refusal frame");
    assert_eq!(refusal["error"]["code"], -32000);
    assert_eq!(refusal["error"]["data"]["code"], "validation_structural");
    assert_eq!(refusal["error"]["data"]["correlation_id"], "look-1");
    let result = find_by_id(&frames, "12").expect("look result");
    assert_eq!(result["result"]["freshness"], "fresh");
    assert_eq!(result["result"]["entries"][0]["seq"], 1);
}

/// Requirement: Look poll responses (lines/offset ranges).
///
/// Present lines/offset values are validated against their own
/// ranges (u32 and u64): valid values are accepted fresh while
/// out-of-range or mistyped values are structural refusals.
#[test]
fn lines_offset_ranges() {
    // (name, lines, offset, expect_fresh).
    let cases: &[(&str, Option<Value>, Option<Value>, bool)] = &[
        ("lines valid, offset omitted", Some(json!(10)), None, true),
        (
            "lines u32 overflow",
            Some(json!(4_294_967_296_u64)),
            None,
            false,
        ),
        ("lines string", Some(json!("5")), None, false),
        ("offset negative", None, Some(json!(-1)), false),
        ("offset valid, lines omitted", None, Some(json!(7)), true),
        ("offset string", None, Some(json!("7")), false),
    ];
    for (name, lines, offset, expect_fresh) in cases {
        let config = test_config(SNAPSHOT);
        let look = look_request(
            "11",
            "look-1",
            Some("h-1"),
            lines.clone(),
            offset.clone(),
            json!(0),
        );
        let input = transcript(&[hello_result(), look, signal("terminate")]);
        let (code, output, _) = run_transcript(&config, &input);
        assert_eq!(code, EXIT_CLEAN, "case {name}");
        let frames = split_frames(&output);
        assert_eq!(frames.len(), 2, "case {name}: {frames:?}");
        let frame = find_by_id(&frames, "11").expect("look frame");
        if *expect_fresh {
            assert_eq!(
                frame["result"]["freshness"], "fresh",
                "case {name}: {frame:?}"
            );
        } else {
            assert_eq!(
                frame["error"]["code"], -32000,
                "case {name}: {frame:?}"
            );
            assert_eq!(
                frame["error"]["data"]["code"], "validation_structural",
                "case {name}: {frame:?}"
            );
            assert_eq!(
                frame["error"]["data"]["correlation_id"], "look-1",
                "case {name}: {frame:?}"
            );
        }
    }
}

/// Requirement: Ping liveness (valid request resets deadline).
///
/// Advances [59000, 5000]: ping 12 observes at 60000, ping 13 at
/// 65000. Both pings answer {}; without the second reset the run at
/// 65000 would sit past the 61000 deadline, but the resets keep it
/// alive to a clean terminate with no liveness_timeout in the sink.
#[test]
fn valid_request_resets() {
    let script = r#"
[clock]
advances_ms = [59000, 5000]
"#;
    let config = test_config(script);
    let input = transcript(&[
        hello_result(),
        ping("12"),
        ping("13"),
        signal("terminate"),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames.len(), 3, "unexpected frames: {frames:?}");
    assert_eq!(
        find_by_id(&frames, "12").expect("ping 12")["result"],
        json!({})
    );
    assert_eq!(
        find_by_id(&frames, "13").expect("ping 13")["result"],
        json!({})
    );
    assert!(
        !has_liveness_timeout(&sink),
        "no liveness expiry after valid resets: {:?}",
        sink.iter()
            .map(|entry| format!("{}:{}", entry.code, entry.detail))
            .collect::<Vec<_>>()
    );
}

/// Requirement: Ping liveness (valid choice result resets deadline).
///
/// Advances [59000, 5000]: submit c-2 observes at 60000, the valid
/// chosen reply observes at 65000. The turn completes Submitted with
/// no liveness_timeout in the sink.
#[test]
fn valid_response_resets() {
    let script = r#"
[clock]
advances_ms = [59000, 5000]

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
    let chosen = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_members("10", "c-2", json!([valid_member("m-1")])),
        chosen,
        signal("terminate"),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let result = find_by_id(&frames, "10").expect("submit result");
    assert_eq!(result["result"]["outcome"], "Submitted");
    assert_eq!(result["result"]["correlation_id"], "c-2");
    assert!(
        !has_liveness_timeout(&sink),
        "no liveness expiry after a valid reply reset: {:?}",
        sink.iter()
            .map(|entry| format!("{}:{}", entry.code, entry.detail))
            .collect::<Vec<_>>()
    );
}

/// Requirement: Ping liveness (admitted error resets deadline).
///
/// Advances [59000, 5000]: submit c-2 observes at 60000, the
/// admitted standard error (no echo required) observes at 65000.
/// The turn completes Submitted with no liveness_timeout.
#[test]
fn error_response_resets() {
    let script = r#"
[clock]
advances_ms = [59000, 5000]

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
    let relay_error = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "error": {"code": -32603, "message": "boom"},
    });
    let input = transcript(&[
        hello_result(),
        submit_members("10", "c-2", json!([valid_member("m-1")])),
        relay_error,
        signal("terminate"),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    let result = find_by_id(&frames, "10").expect("submit result");
    assert_eq!(result["result"]["outcome"], "Submitted");
    assert_eq!(result["result"]["correlation_id"], "c-2");
    assert!(
        !has_liveness_timeout(&sink),
        "no liveness expiry after an admitted error reset: {:?}",
        sink.iter()
            .map(|entry| format!("{}:{}", entry.code, entry.detail))
            .collect::<Vec<_>>()
    );
}

/// Requirement: Ping liveness (fence-hint notifications reset deadline while terminalizing locally).
///
/// Advances [59000, 5000]: two fence-hint supervision signals
/// observe at 60000 and 65000. Each terminalizes locally and
/// extends the deadline: EXIT_CLEAN with no liveness_timeout, and
/// terminalize entries for both fences plus the final terminate.
#[test]
fn notification_resets() {
    let script = r#"
[clock]
advances_ms = [59000, 5000]
"#;
    let config = test_config(script);
    let input = transcript(&[
        hello_result(),
        signal("fence-hint"),
        signal("fence-hint"),
        signal("terminate"),
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let _ = split_frames(&output);
    assert!(
        !has_liveness_timeout(&sink),
        "no liveness expiry after notification resets: {:?}",
        sink.iter()
            .map(|entry| format!("{}:{}", entry.code, entry.detail))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        terminalize_details(&sink),
        vec!["fence_hint", "fence_hint", "terminate"],
        "fences terminalize locally; only the final terminate ends the run"
    );
}

/// Requirement: Ping liveness (an unrecognized supervision signal is not a valid liveness frame).
///
/// The signal domain is closed (terminate|fence-hint): "hold" is a
/// framing failure even with a valid binding, so advances [59000,
/// 2000] cross the original deadline and the session expires
/// instead of draining to EOF.
#[test]
fn unknown_supervision_signal_does_not_reset_liveness() {
    let script = r#"
[clock]
advances_ms = [59000, 2000]
"#;
    let config = test_config(script);
    let bare = json!({"jsonrpc": "2.0"});
    let input = transcript(&[hello_result(), signal("hold"), bare]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        has_liveness_timeout(&sink),
        "expiry, not EOF drain: {:?}",
        sink.iter()
            .map(|entry| format!("{}:{}", entry.code, entry.detail))
            .collect::<Vec<_>>()
    );
    assert!(
        sink.iter()
            .any(|entry| entry.code == "unknown_supervision_signal"),
        "invalid signal is diagnosed: {:?}",
        sink.iter()
            .map(|entry| format!("{}:{}", entry.code, entry.detail))
            .collect::<Vec<_>>()
    );
}

/// Requirement: Ping liveness (malformed reply never resets deadline).
///
/// Advances [30000, 60000]: submit c-2 observes at 31000 (deadline
/// 91000); the both-keys malformed choice reply at 91000 is past that
/// deadline and records without resetting, so the run expires instead
/// of draining to EOF. Expiry at the post-submit deadline proves the
/// malformed reply did not reset it; the held turn never completes,
/// so no submit result for c-2 exists.
///
/// NOTE (spec arithmetic correction): the frozen contract resets
/// liveness on ANY valid frame, so the valid submit moves the
/// deadline to 31000+60000=91000; asserting expiry at the original
/// 61000 deadline is arithmetically impossible (62000 < 91000 drains
/// to EOF, verified empirically: terminalize:eof, no liveness entry).
/// The advances here preserve the test's falsifying power under the
/// correct deadline: a reset-by-malformed implementation would move
/// the deadline to 151000 and drain to EOF instead of expiring.
#[test]
fn malformed_reply_flood_expires() {
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
    let both_keys = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {
                "chosen": {"option_id": "yes", "decided_by": "op-9"},
                "cancelled": {"reason_code": "x"},
            },
        },
    });
    let input = transcript(&[
        hello_result(),
        submit_members("10", "c-2", json!([valid_member("m-1")])),
        both_keys,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let codes: Vec<&str> =
        sink.iter().map(|entry| entry.code.as_str()).collect();
    assert!(
        codes.contains(&"malformed_reply_result"),
        "malformed reply recorded: {codes:?}"
    );
    assert!(
        has_liveness_timeout(&sink),
        "malformed reply did not reset the original deadline: {codes:?} {:?}",
        sink.iter()
            .map(|entry| format!("{}:{}", entry.code, entry.detail))
            .collect::<Vec<_>>()
    );
    let frames = split_frames(&output);
    assert!(
        find_by_id(&frames, "10").is_none(),
        "held turn never completes: {frames:?}"
    );
}
