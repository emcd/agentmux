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

/// A malformed request never resets liveness: a look with empty
/// params carries no binding triple, so the deadline stands and a
/// clock advance past it expires the session — even though the
/// refusal itself is answered on the wire.
#[test]
fn malformed_request_does_not_reset_liveness() {
    let script = r#"
[clock]
advances_ms = [59000, 2000]
"#;
    let config = test_config(script);
    let bad_look = json!({
        "jsonrpc": "2.0",
        "id": "12",
        "method": "relay.look",
        "params": {},
    });
    let bare = json!({"jsonrpc": "2.0"});
    // hello binds at 1000 (deadline 61000). The malformed look
    // consumes advances[0] (now 60000) without resetting; the bare
    // marker consumes advances[1] (now 62000) without resetting; the
    // wait then finds the original deadline passed.
    let input = transcript(&[hello_result(), bad_look, bare]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "liveness_timeout"),
        "expiry, not EOF drain: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    assert!(
        frames
            .iter()
            .any(|frame| frame["error"]["data"]["code"] == "binding_mismatch"),
        "the refusal is still answered: {frames:?}"
    );
}

/// The negotiated look cap bounds concurrently held prime-waits:
/// with max_looks_in_flight 1, a second delayed look is refused with
/// a typed concurrency error while the first is held; once the first
/// resolves, a third is admitted again. Refusal retires the refused
/// call; only held waits occupy the cap.
#[test]
fn look_cap_bounds_held_prime_waits() {
    let script = r#"
[clock]
advances_ms = [250, 300]

[[snapshots]]
target = "seat-1"
delay_ms = 200

[[snapshots.entries]]
kind = "text"
seq = 1
text = "slow hello"
"#;
    let mut config = test_config(script);
    config.proposal.max_looks_in_flight = 1;
    let mut limits = limits_value();
    limits["max_looks_in_flight"] = json!(1);
    let hello = json!({
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
    });
    let look = |id: &str, correlation: &str, handle: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "relay.look",
            "params": {
                "protocol_major": 2,
                "protocol_minor": 1,
                "runtime_instance": "rt-9",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "correlation_id": correlation,
                "look_handle_id": handle,
                "prime_timeout_ms": 2000,
            },
        })
    };
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
    // look-1 (adv 250, anchor 1250, held: 1250 < 1450). look-2
    // (adv 300, now 1550) finds the cap occupied and is refused. The
    // sweep after look-2 answers look-1 fresh (1550 >= 1450), freeing
    // the cap; look-3 is admitted and held (anchor 1550 < 1750);
    // terminate resolves it fence-terminalized.
    let input = transcript(&[
        hello,
        look("11", "look-1", "h-1"),
        look("12", "look-2", "h-2"),
        look("13", "look-3", "h-3"),
        terminate,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames.len(), 4, "unexpected frames: {frames:?}");
    // Refusal first (dispatched before the sweep), then the freed
    // first look answered fresh, then the third look fence-resolved.
    assert_eq!(frames[1]["error"]["code"], -32000);
    assert_eq!(
        frames[1]["error"]["data"]["code"],
        "look_concurrency_exceeded"
    );
    assert_eq!(frames[1]["error"]["data"]["correlation_id"], "look-2");
    assert_eq!(frames[2]["result"]["freshness"], "fresh");
    assert_eq!(frames[2]["result"]["correlation_id"], "look-1");
    assert_eq!(
        frames[3]["result"]["stale_reason_code"],
        "fence_terminalized"
    );
    assert_eq!(frames[3]["result"]["correlation_id"], "look-3");
    assert!(
        !sink_codes(&sink).contains(&"liveness_timeout"),
        "no expiry: {:?}",
        sink_codes(&sink)
    );
}

/// A binding-mismatched request never resets liveness either: a
/// well-formed frame for a foreign runtime is not a valid frame for
/// this binding, so the deadline stands past a crossing advance.
#[test]
fn foreign_binding_request_does_not_reset_liveness() {
    let script = r#"
[clock]
advances_ms = [59000, 2000]
"#;
    let config = test_config(script);
    let foreign = json!({
        "jsonrpc": "2.0",
        "id": "12",
        "method": "relay.ping",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-foreign",
            "target_session": "seat-1",
            "adapter_generation": 3,
        },
    });
    let bare = json!({"jsonrpc": "2.0"});
    let input = transcript(&[hello_result(), foreign, bare]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "liveness_timeout"),
        "expiry, not EOF drain: {:?}",
        sink_codes(&sink)
    );
}

/// Many small frames within a small RX queue all dispatch in
/// order: the rendezvous-depth read channel parks the reader instead
/// of growing memory, and the run loop keeps draining, so no wedge
/// forms under a burst the per-frame bound admits.
#[test]
fn burst_within_rx_queue_all_dispatch_in_order() {
    let config = test_config(SCRIPT);
    let mut limits = limits_value();
    limits["queue_rx_bytes"] = json!(1000);
    let hello = json!({
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
    });
    let mut frames = vec![hello];
    for n in 0..20 {
        frames.push(submit_request(&format!("{}", 10 + n), &format!("c-{n}")));
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
            "signal": "terminate",
        },
    }));
    let input = transcript(&frames);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let out = split_frames(&output);
    let submitted: Vec<&str> = out
        .iter()
        .filter(|frame| frame["result"]["outcome"] == "Submitted")
        .map(|frame| frame["result"]["correlation_id"].as_str().unwrap_or("?"))
        .collect();
    let expected: Vec<String> = (0..20).map(|n| format!("c-{n}")).collect();
    let expected_refs: Vec<&str> =
        expected.iter().map(String::as_str).collect();
    assert_eq!(submitted, expected_refs);
}

/// An unknown method with empty params never resets liveness: with
/// no common binding the frame proves nothing about the peer, so a
/// crossing advance expires the session.
#[test]
fn unknown_method_without_binding_does_not_reset_liveness() {
    let script = r#"
[clock]
advances_ms = [59000, 2000]
"#;
    let config = test_config(script);
    let unknown = json!({
        "jsonrpc": "2.0",
        "id": "12",
        "method": "relay.nope",
        "params": {},
    });
    let bare = json!({"jsonrpc": "2.0"});
    let input = transcript(&[hello_result(), unknown, bare]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "liveness_timeout"),
        "expiry, not EOF drain: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    assert!(
        frames.iter().any(|frame| frame["error"]["code"] == -32601),
        "unknown method still answered: {frames:?}"
    );
}

/// A malformed error for an unknown id never resets liveness: the
/// unknown-id discard is correct, but counting malformed bytes as a
/// valid frame is not.
#[test]
fn malformed_error_on_unknown_id_does_not_reset_liveness() {
    let script = r#"
[clock]
advances_ms = [59000, 2000]
"#;
    let config = test_config(script);
    let bad = json!({
        "jsonrpc": "2.0",
        "id": "99",
        "error": {
            "code": "not-an-integer",
            "message": "bogus",
        },
    });
    let bare = json!({"jsonrpc": "2.0"});
    let input = transcript(&[hello_result(), bad, bare]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter()
            .any(|entry| entry.code == "malformed_reply_error"),
        "malformed error recorded: {:?}",
        sink_codes(&sink)
    );
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "liveness_timeout"),
        "expiry, not EOF drain: {:?}",
        sink_codes(&sink)
    );
}

/// A choice-shaped result missing its required correlation echo is
/// malformed for liveness: results have no echo exemption. The call
/// correctly stays open, but the deadline must stand — a crossing
/// advance then expires instead of draining to EOF.
#[test]
fn result_without_required_echo_does_not_reset_liveness() {
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
    // Well-formed choice outcome but no correlation_id echo.
    let no_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    // Valid submit at 31000 sets the deadline to 91000; the echoless
    // reply at 91000 must not reset it.
    let input = transcript(&[hello_result(), submit, no_echo]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "liveness_timeout"),
        "expiry, not EOF drain: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    assert!(
        frames.iter().all(|frame| frame.get("result").is_none()
            || frame["result"].get("outcome").is_none()),
        "no submit terminal from the echoless reply: {frames:?}"
    );
}

/// An unknown method with correctly typed but FOREIGN binding fields
/// never resets liveness: field types alone are not validity, and
/// the triple never calls the binding equality check it would fail.
#[test]
fn unknown_method_with_foreign_binding_does_not_reset_liveness() {
    let script = r#"
[clock]
advances_ms = [59000, 2000]
"#;
    let config = test_config(script);
    let foreign = json!({
        "jsonrpc": "2.0",
        "id": "12",
        "method": "relay.nope",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "foreign-runtime",
            "target_session": "foreign-target",
            "adapter_generation": 999,
        },
    });
    let bare = json!({"jsonrpc": "2.0"});
    let input = transcript(&[hello_result(), foreign, bare]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "liveness_timeout"),
        "expiry, not EOF drain: {:?}",
        sink_codes(&sink)
    );
    let frames = split_frames(&output);
    assert!(
        frames.iter().any(|frame| frame["error"]["code"] == -32601),
        "unknown method still answered: {frames:?}"
    );
}

/// An unknown-id persist-shaped result without its required echo is
/// malformed, not valid: results have no echo exemption even when
/// their wire names no open entry. Discarding is correct; resetting
/// liveness on the missing-required-field result is not.
#[test]
fn unknown_id_result_without_echo_does_not_reset_liveness() {
    let script = r#"
[clock]
advances_ms = [30000, 60000]

[[turns]]
correlation = "c-1"
ack = "submitted"
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
            "correlation_id": "c-1",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "text",
                "payload_b64": "aGk=",
            }],
        },
    });
    // Well-formed persist shape but no correlation_id echo on an
    // unknown wire id.
    let no_echo = json!({
        "jsonrpc": "2.0",
        "id": "99",
        "result": {"persisted": true},
    });
    // Valid submit at 31000 sets the deadline to 91000; the echoless
    // reply at 91000 must not reset it.
    let input = transcript(&[hello_result(), submit, no_echo]);
    let (code, _, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    assert!(
        sink.iter()
            .any(|entry| entry.code == "malformed_reply_result"),
        "echoless result is malformed: {:?}",
        sink_codes(&sink)
    );
    assert!(
        sink.iter().any(|entry| entry.code == "terminalize"
            && entry.detail == "liveness_timeout"),
        "expiry, not EOF drain: {:?}",
        sink_codes(&sink)
    );
}

/// A held prime-wait answers the requested window once its snapshot
/// primes: with four scripted entries, lines=2 offset=1 held past
/// the 200ms delay resolves fresh with seqs 2..3, entries_total 4,
/// returned 2, and scripted flags preserved. Immediate and held
/// windows share one slicing path.
#[test]
fn held_look_answers_requested_window() {
    let script = r#"
[clock]
advances_ms = [250, 300]

[[snapshots]]
target = "seat-1"
delay_ms = 200

[[snapshots.entries]]
kind = "text"
seq = 1
text = "one"

[[snapshots.entries]]
kind = "text"
seq = 2
text = "two"

[[snapshots.entries]]
kind = "text"
seq = 3
text = "three"

[[snapshots.entries]]
kind = "text"
seq = 4
text = "four"
"#;
    let config = test_config(script);
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
            "prime_timeout_ms": 2000,
            "lines": 2,
            "offset": 1,
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
    // Look dispatch consumes advances[0] (now 1250, anchor 1250,
    // held: 1250 < 1450). Ping consumes advances[1] (now 1550); the
    // sweep after ping answers the held look fresh and windowed.
    let input = transcript(&[hello_result(), look, ping, terminate]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames.len(), 3, "unexpected frames: {frames:?}");
    assert_eq!(frames[1]["result"], json!({}));
    assert_eq!(frames[2]["result"]["freshness"], "fresh");
    assert_eq!(frames[2]["result"]["entries_total"], 4);
    assert_eq!(frames[2]["result"]["returned_entries_count"], 2);
    let seqs: Vec<u64> = frames[2]["result"]["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|entry| entry["seq"].as_u64().expect("seq"))
        .collect();
    assert_eq!(seqs, vec![2, 3]);
    assert!(
        frames[2]["result"]["entries"]
            .as_array()
            .expect("array")
            .iter()
            .all(|entry| entry["truncated"] == false),
        "window cuts never rewrite entry content: {:?}",
        frames[2]["result"]["entries"]
    );
}
