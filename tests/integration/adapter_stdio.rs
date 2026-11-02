use std::collections::HashMap;
use std::io::BufReader;

use agentmux::adapter::choice::DiagnosticEntry;
use agentmux::adapter::frame::BOOTSTRAP_MAX_FRAME_BYTES;
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
    let mut writer = Vec::new();
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    let code = run_stdio(config, &clock, reader, &mut writer, &mut sink);
    (code, writer, sink)
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

/// A full hello/submit/look/ping/terminate run terminates cleanly and
/// the submit resolves Submitted with evidence.
#[test]
fn full_run_terminates_clean() {
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
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames.len(), 6, "unexpected frames: {frames:?}");
    // 0: hello request carries the pinned launch inputs.
    assert_eq!(frames[0]["method"], "adapter.handshakeHello");
    assert_eq!(frames[0]["params"]["target_session"], "seat-1");
    // 1: scripted ready event emits as a lifecycle notification.
    assert_eq!(frames[1]["method"], "adapter.lifecycleEvent");
    assert_eq!(frames[1]["params"]["kind"], "ready");
    // 2: submit resolves Submitted; evidence only on non-Submitted.
    assert_eq!(frames[2]["result"]["outcome"], "Submitted");
    assert_eq!(frames[2]["result"]["correlation_id"], "c-1");
    assert!(frames[2]["result"].get("evidence_basis").is_none());
    // 3: persist marker follows the completed turn.
    assert_eq!(frames[3]["method"], "adapter.persistState");
    // 4: look renders the scripted snapshot as live.
    assert_eq!(frames[4]["result"]["freshness"], "fresh");
    assert_eq!(frames[4]["result"]["entries"][0]["seq"], 1);
    // 5: ping answers the empty object.
    assert_eq!(frames[5]["result"], json!({}));
}

/// The same input trace replays byte-identical: the determinism gate.
#[test]
fn replay_is_byte_identical() {
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
    let input =
        transcript(&[hello_result(), submit_request("10", "c-1"), terminate]);
    let (first_code, first, _) = run_transcript(&config, &input);
    let (second_code, second, _) = run_transcript(&config, &input);
    assert_eq!((first_code, second_code), (EXIT_CLEAN, EXIT_CLEAN));
    assert_eq!(first, second);
}

/// A scripted choice raises requestChoice; the turn completes only
/// after the relay answers it.
#[test]
fn scripted_choice_round_trips_before_submit_completes() {
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
    let chosen = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
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
        submit_request("10", "c-2"),
        chosen,
        terminate,
    ]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    // hello, requestChoice, submit result, persist marker.
    assert_eq!(frames[1]["method"], "adapter.requestChoice");
    assert_eq!(frames[1]["params"]["title"], "Deploy?");
    assert_eq!(frames[2]["result"]["outcome"], "Submitted");
    assert_eq!(frames[2]["result"]["correlation_id"], "c-2");
    assert_eq!(frames[3]["method"], "adapter.persistState");
}

/// A fence-hint with a pending submit terminalizes locally: the
/// choice cancels, the submit records unknown, entries retire.
#[test]
fn fence_with_pending_choice_terminalizes_locally() {
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
    let fence = json!({
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
        submit_request("10", "c-2"),
        fence,
        terminate,
    ]);
    let (code, output, sink) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    // hello, requestChoice; no submit result: the turn never completed.
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1]["method"], "adapter.requestChoice");
    let codes: Vec<&str> =
        sink.iter().map(|entry| entry.code.as_str()).collect();
    assert!(codes.contains(&"terminalize"), "sink: {codes:?}");
    assert!(codes.contains(&"choice_cancelled"), "sink: {codes:?}");
    assert!(codes.contains(&"submission_unknown"), "sink: {codes:?}");
}

/// A scripted snapshot with a delay answers stale-tagged priming
/// until the delay elapses after the target's first poll.
#[test]
fn delayed_snapshot_priming_then_fresh() {
    let script = r#"
[[snapshots]]
target = "seat-1"
delay_ms = 200

[[snapshots.entries]]
kind = "text"
seq = 1
text = "slow hello"
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
    // Fixed clock: the first poll anchors availability 200ms out, so
    // the answer stays stale-tagged priming without blocking traffic.
    let input = transcript(&[hello_result(), look, terminate]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames[1]["result"]["freshness"], "stale");
    assert_eq!(frames[1]["result"]["snapshot_source"], "synthetic");
    assert_eq!(frames[1]["result"]["stale"], true);
    assert_eq!(frames[1]["result"]["stale_reason_code"], "priming");
}

/// A submit naming an unbound member target is refused structurally
/// and holds no turn: the next well-formed submit proceeds.
#[test]
fn wrong_member_target_is_structural() {
    let config = test_config(SCRIPT);
    let bad = json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-bad",
            "members": [{
                "message_id": "m-1",
                "target_session": "elsewhere",
                "adapter_generation": 3,
                "content_type": "default",
                "payload_b64": "aGk=",
            }],
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
        bad,
        submit_request("11", "c-1"),
        terminate,
    ]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    // hello, structural refusal, submit result, persist marker.
    assert_eq!(frames[2]["error"]["code"], -32000);
    assert_eq!(frames[2]["error"]["data"]["code"], "validation_structural");
    assert_eq!(frames[2]["error"]["data"]["correlation_id"], "c-bad");
    assert_eq!(frames[3]["result"]["outcome"], "Submitted");
}

/// An unknown content type is a structural refusal, never a default
/// latency.
#[test]
fn unknown_content_type_is_structural() {
    let config = test_config(SCRIPT);
    let bad = json!({
        "jsonrpc": "2.0",
        "id": "10",
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-bad",
            "members": [{
                "message_id": "m-1",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "nope",
                "payload_b64": "aGk=",
            }],
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
    let input = transcript(&[hello_result(), bad, terminate]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert_eq!(frames[2]["error"]["code"], -32000);
    assert_eq!(frames[2]["error"]["data"]["code"], "validation_structural");
}

/// A choice result with a mismatched correlation echo is kept open:
/// only the correctly echoed reply resolves the turn.
#[test]
fn mismatched_choice_echo_kept_open() {
    let script = r#"
[[turns]]
correlation = "c-2"
ack = "submitted"

[[turns.choices]]
title = "Deploy?"
species = "approval"

[[turns.choices.options]]
option_id = "yes"
name = "Yes"
species = "confirm"
"#;
    let config = test_config(script);
    let wrong_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-WRONG",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
        },
    });
    let right_echo = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes", "decided_by": "op-9"}},
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
        submit_request("10", "c-2"),
        wrong_echo,
        right_echo,
        terminate,
    ]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    // hello, requestChoice, submit result (only after the right echo),
    // persist marker. The mismatched echo resolves nothing.
    assert_eq!(frames[1]["method"], "adapter.requestChoice");
    assert_eq!(frames[2]["result"]["outcome"], "Submitted");
    assert_eq!(frames[2]["result"]["correlation_id"], "c-2");
}

/// A SubmitUnknown turn never releases the gate: the next submit is
/// refused until cessation is observed. Fence hints do not release it.
#[test]
fn unknown_turn_holds_gate_across_fence() {
    let script = r#"
[[turns]]
correlation = "c-u"
ack = "submitted"

[[turns.choices]]
title = "Deploy?"
species = "approval"

[[turns.choices.options]]
option_id = "yes"
name = "Yes"
species = "confirm"
"#;
    let config = test_config(script);
    let submit = |id: &str, correlation: &str| {
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
                    "content_type": "default",
                    "payload_b64": "aGk=",
                }],
            },
        })
    };
    let malformed_choice = json!({
        "jsonrpc": "2.0",
        "id": "2",
        "result": {
            "correlation_id": "ch-2",
            "outcome": {"chosen": {"option_id": "yes"}},
        },
    });
    let fence = json!({
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
    let terminate = json!({
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
    let input = transcript(&[
        hello_result(),
        submit("10", "c-u"),
        malformed_choice,
        submit("11", "c-next"),
        fence,
        submit("12", "c-after-fence"),
        terminate,
    ]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    // hello, requestChoice, submit(SubmissionUnknown on malformed
    // choice), persist marker, busy, busy. No second requestChoice
    // ever goes out: the unknown turn holds the gate across the fence.
    assert_eq!(frames[1]["method"], "adapter.requestChoice");
    assert_eq!(frames[2]["result"]["outcome"], "SubmissionUnknown");
    assert_eq!(frames[2]["result"]["correlation_id"], "c-u");
    assert_eq!(frames[4]["result"]["outcome"], "RefusedBusy");
    assert_eq!(frames[5]["result"]["outcome"], "RefusedBusy");
    assert_eq!(frames.len(), 6, "unexpected frames: {frames:?}");
}

/// The bootstrap ceiling is the 64KiB pre-negotiation bound.
#[test]
fn bootstrap_bound_is_64kib() {
    assert_eq!(BOOTSTRAP_MAX_FRAME_BYTES, 65536);
}
