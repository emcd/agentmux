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
            max_frame_bytes: 67_108_864,
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

fn hello_result_with(frame_cap: u64, rx: u64) -> Value {
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
            "negotiated_limits": {
                "max_frame_bytes": frame_cap,
                "queue_tx_bytes": 16777216,
                "queue_rx_bytes": rx,
                "max_control_pending": 32,
                "max_events_per_second": 100,
                "window_seconds": 1,
                "max_submits_in_flight": 64,
                "max_choices_in_flight": 16,
                "max_looks_in_flight": 8,
                "drain_wait_ms": 250,
            },
        },
    })
}

/// Builds a submit whose serialized body (before the line
/// terminator) is exactly `body_len` bytes, by padding the
/// message id (an opaque string the contract never inspects).
fn exact_submit(id: &str, body_len: usize) -> Vec<u8> {
    let mut frame = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "adapter.submit",
        "params": {
            "protocol_major": 2,
            "protocol_minor": 1,
            "runtime_instance": "rt-9",
            "target_session": "seat-1",
            "adapter_generation": 3,
            "correlation_id": "c-exact",
            "members": [{
                "message_id": "m",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "content_type": "text",
                "payload_b64": "aGk=",
            }],
        },
    });
    let probe = serde_json::to_vec(&frame).expect("serializes");
    assert!(
        probe.len() < body_len,
        "body target must exceed the base frame"
    );
    let pad = "x".repeat(body_len - probe.len());
    frame["params"]["members"][0]["message_id"] = json!(format!("m{pad}"));
    let bytes = serde_json::to_vec(&frame).expect("serializes");
    assert_eq!(bytes.len(), body_len, "exact body size");
    bytes
}

fn transcript_bytes(frames: &[Vec<u8>]) -> Vec<u8> {
    let mut wire = Vec::new();
    for frame in frames {
        wire.extend_from_slice(frame);
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

fn terminate() -> Vec<u8> {
    serde_json::to_vec(&json!({
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
    }))
    .expect("serializes")
    .into_iter()
    .chain(std::iter::once(b'\n'))
    .collect()
}

const SCRIPT: &str = r#"
[[turns]]
correlation = "c-exact"
ack = "submitted"
"#;

/// FRAMING (adapter-wire-schema): a frame whose body is exactly the
/// negotiated bound is admitted with an LF terminator.
#[test]
fn exact_bound_lf_accepted() {
    let config = test_config(SCRIPT);
    let mut hello = serde_json::to_vec(&hello_result_with(4096, 16777216))
        .expect("serializes");
    hello.push(b'\n');
    let mut submit = exact_submit("10", 4096);
    submit.push(b'\n');
    let input = transcript_bytes(&[hello, submit, terminate()]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        frames
            .iter()
            .any(|frame| frame["result"]["outcome"] == "Submitted"),
        "exact-bound frame admitted: {frames:?}"
    );
}

/// FRAMING (adapter-wire-schema): a frame whose body is exactly the
/// negotiated bound is admitted with a CRLF terminator — the
/// carriage return is terminator syntax, not content, and the
/// post-strip body still fits.
#[test]
fn exact_bound_crlf_accepted() {
    let config = test_config(SCRIPT);
    let mut hello = serde_json::to_vec(&hello_result_with(4096, 4096))
        .expect("serializes");
    hello.push(b'\n');
    let mut submit = exact_submit("10", 4096);
    // CRLF terminator on the exact-bound body: the carriage return
    // is terminator syntax, stripped before the bound applies.
    submit.extend_from_slice(b"\r\n");
    let input = transcript_bytes(&[hello, submit, terminate()]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        frames
            .iter()
            .any(|frame| frame["result"]["outcome"] == "Submitted"),
        "exact-bound CRLF frame admitted: {frames:?}"
    );
}

/// FRAMING (adapter-wire-schema): one byte past the bound is
/// discarded with an LF terminator and the session continues — the
/// refused frame terminalizes nothing and the terminate signal ends
/// the session clean.
#[test]
fn over_bound_lf_refused() {
    let config = test_config(SCRIPT);
    let mut hello = serde_json::to_vec(&hello_result_with(4096, 16777216))
        .expect("serializes");
    hello.push(b'\n');
    let mut submit = exact_submit("10", 4097);
    submit.push(b'\n');
    let input = transcript_bytes(&[hello, submit, terminate()]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        frames
            .iter()
            .all(|frame| frame["result"]["outcome"] != "Submitted"),
        "no terminal for the discarded frame: {frames:?}"
    );
}

/// FRAMING (adapter-wire-schema): one byte past the bound is
/// discarded with a CRLF terminator and the session continues — the
/// refused frame terminalizes nothing and the terminate signal ends
/// the session clean.
#[test]
fn over_bound_crlf_refused() {
    let config = test_config(SCRIPT);
    let mut hello = serde_json::to_vec(&hello_result_with(4096, 4096))
        .expect("serializes");
    hello.push(b'\n');
    let mut submit = exact_submit("10", 4097);
    submit.extend_from_slice(b"\r\n");
    let input = transcript_bytes(&[hello, submit, terminate()]);
    let (code, output, _) = run_transcript(&config, &input);
    assert_eq!(code, EXIT_CLEAN);
    let frames = split_frames(&output);
    assert!(
        frames
            .iter()
            .all(|frame| frame["result"]["outcome"] != "Submitted"),
        "no terminal for the discarded frame: {frames:?}"
    );
}

/// FRAMING (adapter-wire-schema): terminators split across transport
/// reads still terminate — a byte-at-a-time peer delivers the same
/// frames as a coalesced one. Drives run_stdio through a 1-byte
/// reader.
#[test]
fn split_delivery_matches_coalesced_delivery() {
    struct OneByte<R>(R);
    impl<R: std::io::Read> std::io::Read for OneByte<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut one = [0u8; 1];
            match self.0.read(&mut one) {
                Ok(0) => Ok(0),
                Ok(_) => {
                    buf[0] = one[0];
                    Ok(1)
                },
                Err(source) => Err(source),
            }
        }
    }
    let config = test_config(SCRIPT);
    let mut hello = serde_json::to_vec(&hello_result_with(4096, 16777216))
        .expect("serializes");
    hello.push(b'\n');
    let mut submit = exact_submit("10", 500);
    submit.extend_from_slice(b"\r\n");
    let term = terminate();
    let mut input = Vec::new();
    input.extend_from_slice(&hello);
    input.extend_from_slice(&submit);
    input.extend_from_slice(&term);
    let clock = ManualClock::new(1_000);
    let reader = OneByte(std::io::Cursor::new(input));
    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = SharedWriter(shared.clone());
    let mut sink: Vec<DiagnosticEntry> = Vec::new();
    let code = run_stdio(&config, &clock, reader, writer, &mut sink);
    assert_eq!(code, EXIT_CLEAN);
    let output = shared.lock().expect("writer lock").clone();
    let frames = split_frames(&output);
    assert!(
        frames
            .iter()
            .any(|frame| frame["result"]["outcome"] == "Submitted"),
        "split delivery matches coalesced: {frames:?}"
    );
}
