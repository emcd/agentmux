//! Transcript replay for the adapter conformance suite.
//!
//! A transcript is a supervisor-side frame script: the binding handshake,
//! then ordered steps that send frames to the adapter and assert the
//! frames it emits. Expectations match by subset — every expected field
//! must equal the actual field, while script-defined extras pass through
//! unchecked — except arrays, which match element-wise at equal length.
//! Placeholder strings of the form `$name` in sent frames substitute ids
//! captured by earlier expectations, so transcripts can answer
//! adapter-originated calls without pinning the adapter's id counter.
//! Large payloads stay out of the fixture files: `expand_b64` fills a
//! pointed-to string field with deterministic filler at run time.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    time::Duration,
};

use serde::Deserialize;
use serde_json::Value;

use super::harness::SupervisorDouble;

/// A wire-id capture referenced by later sent frames as `$name`.
const PLACEHOLDER_PREFIX: &str = "$";

/// How long each expectation waits for its frame.
const STEP_DEADLINE: Duration = Duration::from_secs(5);

/// One transcript fixture: binding plus ordered replay steps.
#[derive(Debug, Deserialize)]
pub(crate) struct Transcript {
    pub(crate) requirement: String,
    pub(crate) capability: String,
    pub(crate) limit: Option<String>,
    pub(crate) description: String,
    pub(crate) adapter_script: String,
    pub(crate) spawn: SpawnHint,
    pub(crate) bind: Bind,
    pub(crate) frames: Vec<Step>,
}

/// How to launch the adapter process under replay.
#[derive(Debug, Deserialize)]
pub(crate) struct SpawnHint {
    #[serde(default)]
    pub(crate) args: Vec<String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
}

/// The handshake the transcript expects, and the result to answer with.
#[derive(Debug, Deserialize)]
pub(crate) struct Bind {
    pub(crate) expect_hello_subset: Value,
    pub(crate) hello_result: Value,
    pub(crate) hello_result_raw: Option<String>,
}

/// One replay step, distinguished by its single key.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Step {
    Send {
        send: Value,
        expand_b64: Option<ExpandB64>,
    },
    SendRaw {
        send_raw: String,
    },
    Expect {
        expect: Value,
        capture_id_as: Option<String>,
        capture_correlation_as: Option<String>,
    },
    ExpectQuiet {
        expect_quiet_ms: u64,
    },
    ExpectEof {
        expect_eof: bool,
    },
    CloseInput {
        close_input: bool,
    },
}

/// Deterministic filler for a base64 string field, keeping large payloads
/// out of the fixture files. The byte count rounds down to a multiple of
/// four so the filler stays valid base64.
#[derive(Debug, Deserialize)]
pub(crate) struct ExpandB64 {
    pub(crate) pointer: String,
    pub(crate) bytes: usize,
}

/// Loads every transcript in `directory`, sorted by file name.
pub(crate) fn load_transcripts(
    directory: &Path,
) -> Result<Vec<(String, Transcript)>, String> {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .map_err(|source| format!("read {}: {source}", directory.display()))?
        .collect::<Result<_, _>>()
        .map_err(|source| format!("list {}: {source}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut transcripts = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|source| format!("read {}: {source}", path.display()))?;
        let transcript: Transcript = serde_json::from_str(&raw)
            .map_err(|source| format!("parse {}: {source}", path.display()))?;
        let name = path
            .file_stem()
            .expect("transcript stem")
            .to_string_lossy()
            .into_owned();
        transcripts.push((name, transcript));
    }
    Ok(transcripts)
}

/// Reports whether `expected` holds inside `actual`: objects match by
/// subset, arrays element-wise at equal length, scalars by equality.
/// One exception keeps the matcher honest: an empty expected object
/// matches only an empty actual object, so a `{}` expectation pins
/// emptiness instead of passing everything.
pub(crate) fn subset_match(
    expected: &Value,
    actual: &Value,
) -> Result<(), String> {
    subset_match_at(expected, actual, "$")
}

fn subset_match_at(
    expected: &Value,
    actual: &Value,
    path: &str,
) -> Result<(), String> {
    match (expected, actual) {
        (Value::Object(expected_map), Value::Object(actual_map)) => {
            if expected_map.is_empty() && !actual_map.is_empty() {
                return Err(format!(
                    "{path}: expected empty object, got {actual_map:?}"
                ));
            }
            for (key, expected_value) in expected_map {
                let actual_value = actual_map
                    .get(key)
                    .ok_or_else(|| format!("{path}.{key}: missing field"))?;
                subset_match_at(
                    expected_value,
                    actual_value,
                    &format!("{path}.{key}"),
                )?;
            }
            Ok(())
        },
        (Value::Array(expected_items), Value::Array(actual_items)) => {
            if expected_items.len() != actual_items.len() {
                return Err(format!(
                    "{path}: expected {} items, got {}",
                    expected_items.len(),
                    actual_items.len(),
                ));
            }
            for (index, (expected_item, actual_item)) in
                expected_items.iter().zip(actual_items.iter()).enumerate()
            {
                subset_match_at(
                    expected_item,
                    actual_item,
                    &format!("{path}[{index}]"),
                )?;
            }
            Ok(())
        },
        (expected_scalar, actual_scalar) => {
            if expected_scalar == actual_scalar {
                Ok(())
            } else {
                Err(format!(
                    "{path}: expected {expected_scalar}, got {actual_scalar}"
                ))
            }
        },
    }
}

/// Replaces `$name` strings in a sent frame with captured wire ids.
fn substitute_captures(
    frame: &mut Value,
    captures: &HashMap<String, String>,
) -> Result<(), String> {
    match frame {
        Value::String(text) => {
            if let Some(name) = text.strip_prefix(PLACEHOLDER_PREFIX) {
                *text = captures.get(name).cloned().ok_or_else(|| {
                    format!("uncaptured placeholder ${name}")
                })?;
            }
            Ok(())
        },
        Value::Array(items) => {
            for item in items {
                substitute_captures(item, captures)?;
            }
            Ok(())
        },
        Value::Object(map) => {
            for value in map.values_mut() {
                substitute_captures(value, captures)?;
            }
            Ok(())
        },
        _ => Ok(()),
    }
}

/// Fills the pointed-to string field with deterministic base64 filler.
fn expand_filler(frame: &mut Value, expand: &ExpandB64) -> Result<(), String> {
    let mut target = frame;
    let mut parts = expand.pointer.split('/').peekable();
    if parts.peek() == Some(&"") {
        parts.next();
    }
    for part in parts {
        target = match target {
            Value::Object(map) => map
                .get_mut(part)
                .ok_or_else(|| format!("expand pointer missing /{part}"))?,
            Value::Array(items) => {
                let index: usize = part.parse().map_err(|_| {
                    format!("expand pointer bad index /{part}")
                })?;
                items.get_mut(index).ok_or_else(|| {
                    format!("expand pointer out of range /{part}")
                })?
            },
            _ => {
                return Err(format!(
                    "expand pointer cannot descend at /{part}"
                ));
            },
        };
    }
    let aligned = expand.bytes - expand.bytes % 4;
    if aligned == 0 {
        return Err("expand bytes rounds to empty filler".to_string());
    }
    match target {
        Value::String(text) => {
            *text = "Q".repeat(aligned);
            Ok(())
        },
        _ => Err("expand pointer names a non-string field".to_string()),
    }
}

/// Replays one transcript against `binary`, failing on the first mismatch.
pub(crate) fn run_transcript(
    binary: &str,
    transcript: &Transcript,
) -> Result<(), String> {
    let environment: Vec<(&str, &str)> = transcript
        .spawn
        .env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let arguments: Vec<&str> =
        transcript.spawn.args.iter().map(String::as_str).collect();
    let mut double =
        SupervisorDouble::spawn_with_env(binary, &arguments, &environment)
            .map_err(|source| format!("spawn adapter: {source}"))?;
    let call = double
        .expect_hello(STEP_DEADLINE)
        .ok_or_else(|| "adapter sent no admittable hello".to_string())?;
    subset_match(&transcript.bind.expect_hello_subset, &call.params)
        .map_err(|detail| format!("hello params mismatch: {detail}"))?;
    match &transcript.bind.hello_result_raw {
        Some(raw) => {
            double.send_raw(&raw.replace("$hello_id", &call.wire_id));
        },
        None => {
            double.answer_hello(&call, transcript.bind.hello_result.clone())
        },
    }
    let mut captures: HashMap<String, String> = HashMap::new();
    for (index, step) in transcript.frames.iter().enumerate() {
        let context = format!("step {index}");
        match step {
            Step::Send { send, expand_b64 } => {
                let mut frame = send.clone();
                substitute_captures(&mut frame, &captures)
                    .map_err(|detail| format!("{context}: {detail}"))?;
                if let Some(expand) = expand_b64 {
                    expand_filler(&mut frame, expand)
                        .map_err(|detail| format!("{context}: {detail}"))?;
                }
                double.send_frame(&frame);
            },
            Step::SendRaw { send_raw } => {
                double.send_raw(send_raw);
            },
            Step::Expect {
                expect,
                capture_id_as,
                capture_correlation_as,
            } => match double.read_frame_deadline(STEP_DEADLINE) {
                super::harness::ReadOutcome::Frame(frame) => {
                    if frame.get("result").is_some()
                        && frame.get("error").is_some()
                    {
                        return Err(format!(
                            "{context}: frame carries both result and error"
                        ));
                    }
                    subset_match(expect, &frame).map_err(|detail| {
                        format!("{context} mismatch: {detail}")
                    })?;
                    if let Some(name) = capture_id_as {
                        let id = frame["id"].as_str().ok_or_else(|| {
                            format!("{context}: captured frame has no id")
                        })?;
                        captures.insert(name.clone(), id.to_string());
                    }
                    if let Some(name) = capture_correlation_as {
                        let correlation = frame["params"]["correlation_id"]
                            .as_str()
                            .ok_or_else(|| {
                                format!(
                                    "{context}: captured frame has no correlation"
                                )
                            })?;
                        captures.insert(name.clone(), correlation.to_string());
                    }
                },
                super::harness::ReadOutcome::Timeout => {
                    return Err(format!("{context}: timed out waiting"));
                },
                super::harness::ReadOutcome::Unparseable(raw) => {
                    return Err(format!(
                        "{context}: unparseable line {raw:?}"
                    ));
                },
                super::harness::ReadOutcome::Incomplete(raw) => {
                    return Err(format!(
                        "{context}: unterminated line {raw:?}"
                    ));
                },
                super::harness::ReadOutcome::ReadFailed(reason) => {
                    return Err(format!("{context}: read failed: {reason}"));
                },
                super::harness::ReadOutcome::Closed => {
                    return Err(format!("{context}: pipe closed early"));
                },
            },
            Step::ExpectQuiet { expect_quiet_ms } => {
                double
                    .expect_quiet(Duration::from_millis(*expect_quiet_ms))
                    .map_err(|detail| format!("{context}: {detail}"))?;
            },
            Step::ExpectEof { expect_eof } => {
                if *expect_eof && !double.expect_eof(STEP_DEADLINE) {
                    return Err(format!("{context}: pipe stayed open"));
                }
            },
            Step::CloseInput { close_input } => {
                if *close_input {
                    double.close_stdin();
                }
            },
        }
    }
    double.disconnect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requirement titles stated by the frozen contract specs.
    fn contract_requirements() -> Vec<String> {
        let specs = Path::new(
            "documentation/architecture/openspec/changes/add-agent-adapter-contract/specs",
        );
        let mut titles = Vec::new();
        let mut directories: Vec<_> = std::fs::read_dir(specs)
            .expect("contract specs directory")
            .collect::<Result<_, _>>()
            .expect("list contract specs");
        directories.sort_by_key(|entry| entry.file_name());
        for directory in directories {
            let path = directory.path().join("spec.md");
            let text =
                std::fs::read_to_string(&path).expect("read contract spec");
            for line in text.lines() {
                if let Some(title) = line.strip_prefix("### Requirement: ") {
                    titles.push(title.to_string());
                }
            }
        }
        titles
    }

    /// Capability file stems stated by the frozen contract change.
    fn contract_capabilities() -> Vec<String> {
        let specs = Path::new(
            "documentation/architecture/openspec/changes/add-agent-adapter-contract/specs",
        );
        let mut names: Vec<_> = std::fs::read_dir(specs)
            .expect("contract specs directory")
            .collect::<Result<Vec<std::fs::DirEntry>, _>>()
            .expect("list contract specs")
            .into_iter()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Methods the supervisor side may originate. Choice results and
    /// errors travel by wire id with no method; the hello answer is
    /// built by the runner, not the fixture.
    const SUPERVISOR_METHODS: &[&str] = &[
        "adapter.submit",
        "relay.look",
        "relay.ping",
        "relay.supervisionSignal",
    ];

    #[test]
    fn transcript_fixtures_cite_contract_requirements() {
        let directory = Path::new("tests/fixtures/adapter/transcripts");
        let transcripts =
            load_transcripts(directory).expect("load transcripts");
        assert!(!transcripts.is_empty(), "no transcript fixtures to check");
        let requirements = contract_requirements();
        let capabilities = contract_capabilities();
        for (name, transcript) in &transcripts {
            assert!(
                requirements
                    .iter()
                    .any(|title| title == &transcript.requirement),
                "{name}: requirement {:?} is not a frozen-contract requirement",
                transcript.requirement,
            );
            assert!(
                capabilities
                    .iter()
                    .any(|stem| stem == &transcript.capability),
                "{name}: capability {:?} is not a frozen-contract spec",
                transcript.capability,
            );
            if let Some(limit) = &transcript.limit {
                assert!(
                    !limit.trim().is_empty() && !limit.contains('\n'),
                    "{name}: limit must be non-empty single-line text",
                );
            }
            assert!(
                !transcript.description.trim().is_empty(),
                "{name}: missing description",
            );
            assert!(
                transcript.adapter_script.ends_with(".toml"),
                "{name}: adapter script must name a TOML fixture",
            );
            assert!(!transcript.frames.is_empty(), "{name}: no frames");
            let mut captured: Vec<&str> = Vec::new();
            if let Some(raw) = &transcript.bind.hello_result_raw {
                for token in raw.split(['"', ' ', ':', ',', '{', '}']) {
                    if let Some(rest) = token.strip_prefix('$') {
                        assert_eq!(
                            rest, "hello_id",
                            "{name}: raw hello may only reference $hello_id",
                        );
                    }
                }
            }
            for step in &transcript.frames {
                match step {
                    Step::Send { send, expand_b64 } => {
                        if let Some(method) =
                            send.get("method").and_then(Value::as_str)
                        {
                            assert!(
                                SUPERVISOR_METHODS.contains(&method),
                                "{name}: supervisor never originates {method}",
                            );
                            if method == "relay.look" {
                                assert!(
                                    send["params"]["prime_timeout_ms"]
                                        .is_number(),
                                    "{name}: look without prime_timeout_ms is a structural refusal, not a poll",
                                );
                            }
                        }
                        if let Some(error) = send.get("error") {
                            assert!(
                                error["code"].is_number(),
                                "{name}: error shape needs a numeric code",
                            );
                        }
                        if let Some(expand) = expand_b64 {
                            assert!(
                                expand.bytes >= 4,
                                "{name}: expand filler rounds to empty",
                            );
                        }
                        check_placeholders(name, send, &captured);
                    },
                    Step::SendRaw { send_raw } => {
                        assert!(
                            !send_raw.trim().is_empty(),
                            "{name}: empty raw line",
                        );
                    },
                    Step::Expect {
                        expect: _,
                        capture_id_as,
                        capture_correlation_as,
                    } => {
                        if let Some(id) = capture_id_as {
                            captured.push(id);
                        }
                        if let Some(correlation) = capture_correlation_as {
                            captured.push(correlation);
                        }
                    },
                    Step::ExpectQuiet { expect_quiet_ms } => {
                        assert!(
                            *expect_quiet_ms > 0,
                            "{name}: quiet wait must be positive",
                        );
                    },
                    Step::ExpectEof { expect_eof } => {
                        assert!(*expect_eof, "{name}: eof expectation denied");
                    },
                    Step::CloseInput { close_input } => {
                        assert!(
                            *close_input,
                            "{name}: close-input step denied"
                        );
                    },
                }
            }
        }
    }

    /// Every `$name` in a sent frame resolves to an earlier capture.
    fn check_placeholders(name: &str, frame: &Value, captured: &[&str]) {
        match frame {
            Value::String(text) => {
                if let Some(variable) = text.strip_prefix('$') {
                    assert!(
                        captured.contains(&variable),
                        "{name}: ${variable} has no earlier capture",
                    );
                }
            },
            Value::Array(items) => {
                for item in items {
                    check_placeholders(name, item, captured);
                }
            },
            Value::Object(map) => {
                for value in map.values() {
                    check_placeholders(name, value, captured);
                }
            },
            _ => {},
        }
    }

    #[test]
    fn subset_matcher_accepts_subsets_and_reports_paths() {
        let actual = serde_json::json!({
            "a": {"b": 1, "c": 2},
            "items": [{"x": 1, "y": 2}],
        });
        assert!(
            subset_match(&serde_json::json!({"a": {"b": 1}}), &actual).is_ok()
        );
        let missing =
            subset_match(&serde_json::json!({"a": {"z": 1}}), &actual)
                .expect_err("missing field must fail");
        assert!(missing.contains("$.a.z"), "path was {missing}");
        let length = subset_match(
            &serde_json::json!({"items": [{"x": 1}, {"x": 2}]}),
            &actual,
        )
        .expect_err("length mismatch must fail");
        assert!(length.contains("$.items"), "path was {length}");
    }

    #[test]
    fn empty_expected_object_matches_only_empty() {
        assert!(
            subset_match(
                &serde_json::json!({"result": {}}),
                &serde_json::json!({"result": {}}),
            )
            .is_ok()
        );
        let nonempty = subset_match(
            &serde_json::json!({"result": {}}),
            &serde_json::json!({"result": {"wrong": true}}),
        )
        .expect_err("nonempty pong must fail an empty expectation");
        assert!(nonempty.contains("$.result"), "path was {nonempty}");
    }

    /// Inline transcript proving the envelope rule against a stub: a
    /// frame carrying both result and error is rejected even though
    /// each half subset-matches on its own.
    #[test]
    fn replay_rejects_frame_with_both_result_and_error() {
        let script = r#"printf '%s\n' '{"jsonrpc":"2.0","id":"1","method":"adapter.handshakeHello","params":{"adapter_profile":"generic","protocol_major":1,"protocol_minor":0}}'; printf '%s\n' '{"jsonrpc":"2.0","id":"9","result":{},"error":{"code":-32600,"message":"x"}}'; read -r first"#;
        let transcript: Transcript = serde_json::from_value(serde_json::json!({
            "requirement": "Newline-delimited JSON-RPC 2.0 framing with contract method mapping",
            "capability": "adapter-wire-schema",
            "limit": null,
            "description": "envelope rule",
            "adapter_script": "no-op.toml",
            "spawn": {"args": ["-c", script], "env": {}},
            "bind": {
                "expect_hello_subset": {"adapter_profile": "generic"},
                "hello_result": {"runtime_instance": "i", "snapshot_b64": ""},
                "hello_result_raw": null
            },
            "frames": [
                {"expect": {"jsonrpc": "2.0", "id": "9"}}
            ]
        }))
        .expect("inline transcript parses");
        let detail = run_transcript("sh", &transcript)
            .expect_err("both result and error must fail");
        assert!(
            detail.contains("both result and error"),
            "detail was {detail}"
        );
    }

    /// Inline transcript proving runner plumbing against a stub: hello
    /// exchange, one ping round-trip.
    #[test]
    fn replay_ping_exchange_through_stub() {
        let script = r#"printf '%s\n' '{"jsonrpc":"2.0","id":"1","method":"adapter.handshakeHello","params":{"adapter_profile":"generic","target_session":"alpha","protocol_major":1,"protocol_minor":0}}'; read -r first; read -r second; printf '%s\n' '{"jsonrpc":"2.0","id":"1","result":{}}'"#;
        let transcript: Transcript = serde_json::from_value(serde_json::json!({
            "requirement": "Newline-delimited JSON-RPC 2.0 framing with contract method mapping",
            "capability": "adapter-wire-schema",
            "limit": null,
            "description": "plumbing",
            "adapter_script": "no-op.toml",
            "spawn": {"args": ["-c", script], "env": {}},
            "bind": {
                "expect_hello_subset": {"adapter_profile": "generic"},
                "hello_result": {"runtime_instance": "i", "snapshot_b64": ""},
                "hello_result_raw": null
            },
            "frames": [
                {"send": {"jsonrpc": "2.0", "id": "1", "method": "relay.ping", "params": {}}},
                {"expect": {"jsonrpc": "2.0", "id": "1", "result": {}}}
            ]
        }))
        .expect("inline transcript parses");
        run_transcript("sh", &transcript).expect("stub replay succeeds");
    }

    /// Inline transcript proving capture, substitution, and quiet
    /// against a stub that raises a canned choice and then holds.
    #[test]
    fn replay_capture_and_quiet_through_stub() {
        let script = r#"printf '%s\n' '{"jsonrpc":"2.0","id":"1","method":"adapter.handshakeHello","params":{"adapter_profile":"generic","protocol_major":1,"protocol_minor":0}}'; printf '%s\n' '{"jsonrpc":"2.0","id":"7","method":"adapter.requestChoice","params":{"correlation_id":"k-1"}}'; read -r first; read -r second; read -r third"#;
        let transcript: Transcript = serde_json::from_value(serde_json::json!({
            "requirement": "Pending-call table scopes results and errors",
            "capability": "adapter-wire-schema",
            "limit": null,
            "description": "plumbing",
            "adapter_script": "choice-raise-hanging.toml",
            "spawn": {"args": ["-c", script], "env": {}},
            "bind": {
                "expect_hello_subset": {"adapter_profile": "generic"},
                "hello_result": {"runtime_instance": "i", "snapshot_b64": ""},
                "hello_result_raw": null
            },
            "frames": [
                {"expect": {"jsonrpc": "2.0", "method": "adapter.requestChoice"}, "capture_id_as": "c1", "capture_correlation_as": "cc1"},
                {"send": {"jsonrpc": "2.0", "id": "$c1", "result": {"correlation_id": "$cc1", "outcome": {"cancelled": {"reason_code": "user-cancelled"}}}}},
                {"expect_quiet_ms": 300}
            ]
        }))
        .expect("inline transcript parses");
        run_transcript("sh", &transcript).expect("stub replay succeeds");
    }

    /// Gate for the real binary. Runs in the normal suite: the binary
    /// target builds alongside the test, so its path is known at
    /// compile time.
    #[test]
    fn replay_transcripts_against_reference_adapter() {
        let binary = env!("CARGO_BIN_EXE_agentmux-adapter");
        let directory = Path::new("tests/fixtures/adapter/transcripts");
        let transcripts =
            load_transcripts(directory).expect("load transcripts");
        assert!(!transcripts.is_empty(), "no transcripts to replay");
        for (name, transcript) in &transcripts {
            run_transcript(binary, transcript)
                .unwrap_or_else(|detail| panic!("{name}: {detail}"));
        }
    }
}
