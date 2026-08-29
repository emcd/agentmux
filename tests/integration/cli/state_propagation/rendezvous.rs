use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

use super::super::super::support::process;
use super::super::helpers::*;
use super::helpers::*;

pub(super) fn write_rendezvous_bundle(
    config_root: &Path,
    report: &Path,
    member_directory: &Path,
    declared: &str,
) {
    write_bundle_configuration_with_options(config_root, "alpha", None, &["a"], Some(false));
    // The descendant is a real `agentmux host mcp`, driven over stdio the way a
    // coder drives it, rather than a CLI client standing in for one. That is the
    // process the propagation contract names, and it resolves association as
    // well as the state root, so both arrive by inheritance or neither does.
    //
    // Written as a script because the JSON-RPC frames would otherwise have to
    // survive TOML quoting inside shell quoting.
    let script = config_root.join("mcp-rendezvous.sh");
    fs::write(
        &script,
        format!(
            r#"#!/usr/bin/env bash
# No --state-directory: reaching the relay is possible only by resolving the
# inherited AGENTMUX_STATE_DIRECTORY.
{{
  printf '%s\n' '{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"rendezvous","version":"0"}}}}}}'
  printf '%s\n' '{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"list","arguments":{{"command":"principals","args":{{}}}}}}}}'
}} | {binary} host mcp --configuration-directory {config} > {report} 2>&1
"#,
            binary = env!("CARGO_BIN_EXE_agentmux"),
            config = config_root.display(),
            report = report.display()
        ),
    )
    .expect("write rendezvous script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
        .expect("make rendezvous script executable");
    fs::write(
        config_root.join("coders.toml"),
        format!(
            r#"
format-version = 1

[[coders]]
id = "default"

[coders.tmux]
initial-command = "sh -lc '{script}; exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
            script = script.display()
        ),
    )
    .expect("write rendezvous coders config");
    // The member runs from a directory that is not the relay's, so a relative or
    // unnormalized state root would resolve somewhere else and fail rather than
    // pass by coincidence.
    let bundle = fs::read_to_string(config_root.join("bundles").join("alpha.toml"))
        .expect("read bundle configuration")
        .replace(
            "directory = \"/tmp\"",
            &format!("directory = \"{}\"", member_directory.display()),
        );
    fs::write(config_root.join("bundles").join("alpha.toml"), bundle)
        .expect("write rendezvous bundle config");
    // The member declares its own state root, which the relay must overwrite. The
    // descendant inherits whatever the member was spawned with, so this is the
    // value that reaches the process doing the resolving.
    declare_member_state_directory(config_root, "alpha", declared);
}

/// Waits for the spawned member's client to finish its second exchange, and
/// returns the decoded responses keyed by request id together with the raw
/// report for diagnostics.
///
/// Waiting on a non-empty file is not enough: the `initialize` response arrives
/// first, and returning on it tears the relay down before the `tools/call` the
/// assertions read. Waiting on the *text* `"id":2` is not enough either — the
/// child's write is not atomic with respect to this reader, so that substring can
/// be present while the line holding it is still partial, which puts the race
/// back where it was one layer down.
///
/// So the wait is for a complete newline-terminated JSON object carrying id 2.
/// Anything the writer has not finished fails to parse and is simply not counted
/// yet. That answer arrives either way — an unreachable relay yields a successful
/// call reporting the bundle down — so this waits for the exchange to finish
/// rather than for it to succeed.
pub(super) fn await_rendezvous_report(report: &Path) -> (HashMap<u64, Value>, String) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_seen = String::new();
    while Instant::now() < deadline {
        if let Ok(contents) = fs::read_to_string(report) {
            let responses = decode_complete_responses(&contents);
            if responses.contains_key(&2) {
                return (responses, contents);
            }
            last_seen = contents;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "the spawned member's agentmux client never completed a response with id 2 at {}; \
         it reported:\n{last_seen}",
        report.display()
    );
}

/// Decodes the complete JSON-RPC responses in `contents`, keyed by request id.
///
/// A trailing partial line, or any non-JSON the member's shell wrote, is skipped
/// rather than treated as an error: this reads a file another process is still
/// appending to.
fn decode_complete_responses(contents: &str) -> HashMap<u64, Value> {
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            let id = value.get("id").and_then(Value::as_u64)?;
            Some((id, value))
        })
        .collect()
}

#[test]
fn a_partially_written_response_line_is_not_counted_as_complete() {
    // The deterministic half of the teardown race. The race itself depends on
    // catching the child mid-write, which does not reproduce on demand, so what
    // is asserted is the property the wait rests on: a line the writer has not
    // finished contributes no id, however much of it is present. Waiting on the
    // text `"id":2` instead would return on the partial line below.
    let complete = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05"}}"#;
    let partial = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","tex"#;
    let responses = decode_complete_responses(&format!("{complete}\n{partial}"));

    assert!(
        responses.contains_key(&1),
        "the finished response must still be counted"
    );
    assert!(
        !responses.contains_key(&2),
        "a partial line must not satisfy the wait, or the relay is torn down \
         before the response the assertions read is written"
    );
}

/// Extracts the bundle entry for `bundle_name` from a `list principals` tool
/// response.
///
/// The payload is JSON encoded as a string inside `result.content[0].text`, so
/// reaching the bundle means decoding two layers. Asserting on the decoded value
/// rather than on the enclosing text is what makes the assertion independent of
/// how the payload happens to be escaped.
fn listed_bundle(response: &Value, bundle_name: &str) -> Value {
    let text = response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("tool response carries no content text: {response}"));
    let payload = serde_json::from_str::<Value>(text)
        .unwrap_or_else(|error| panic!("decode tool payload {text}: {error}"));
    payload
        .get("bundles")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("tool payload carries no bundles array: {payload}"))
        .iter()
        .find(|bundle| bundle.get("id").and_then(Value::as_str) == Some(bundle_name))
        .unwrap_or_else(|| panic!("tool payload lists no bundle {bundle_name}: {payload}"))
        .clone()
}

/// A relay-spawned member's own agentmux client reaches the relay that spawned
/// it, resolving the state root from inherited environment alone, whatever the
/// member declared for itself.
///
/// This is the propagation contract end to end rather than by argv inspection:
/// the child is a real process, started by the relay through tmux, given no
/// state directory of its own, and it has to arrive at the right socket. If the
/// stamp were missing the child would resolve the XDG or home default, find no
/// relay, and report the bundle down.
fn assert_rendezvous_survives_declaration(declared: &str) {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    let member_directory = temporary.path().join("member-cwd");
    let report = temporary.path().join("rendezvous-report");
    // Where the spawned child's *fallback* tier lands if the stamp fails to
    // arrive. Pointing it inside the fixture is a safety requirement, not
    // tidiness: on a developer machine the real XDG state root often holds a
    // live relay, and a broken stamp would otherwise send this test's child to
    // connect to it. It also makes the negative case deterministic — no relay
    // there, ever — instead of depending on what happens to be running.
    let isolated_fallback = temporary.path().join("fallback-state");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    fs::create_dir_all(&member_directory).expect("create member directory");
    fs::create_dir_all(&isolated_fallback).expect("create fallback state root");
    write_rendezvous_bundle(&config_root, &report, &member_directory, declared);

    // Real tmux, not the fake: the stamp has to reach a live child, and the fake
    // records invocations without executing the member's command.
    let mut relay_command = Command::new(env!("CARGO_BIN_EXE_agentmux"));
    // Inherited Agentmux context is stripped before the fixture's own values are
    // applied. `XDG_STATE_HOME` below contains only the *default* tier; an
    // inherited `AGENTMUX_STATE_DIRECTORY` outranks it, so leaving one in place
    // would let a suite run from an Agentmux-launched coder send this test's
    // descendant to the developer's own live relay.
    let host_child = process::RelayChildGuard::new(
        process::strip_bring_up_context_std(&mut relay_command)
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            // The relay is unaffected (it was given an explicit state directory);
            // this is inherited by the tmux server and thence by the member's child,
            // which is the process whose fallback needs containing.
            .env("XDG_STATE_HOME", &isolated_fallback)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
            "alpha",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux up");
    // Both streams, because the cause is not on the one that carries the
    // failure: `up` reports the top-level error on stderr, but the per-bundle
    // `reason_code` and the per-session `cause` are printed to stdout. Reporting
    // stderr alone leaves a CI failure saying that the transition failed and
    // nothing about why.
    assert!(
        up.status.success(),
        "up should start the member; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&up.stdout),
        String::from_utf8_lossy(&up.stderr)
    );

    let (responses, reported) = await_rendezvous_report(&report);

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();

    let initialized = responses
        .get(&1)
        .unwrap_or_else(|| panic!("no initialize response; it reported:\n{reported}"));
    assert!(
        initialized
            .pointer("/result/protocolVersion")
            .and_then(Value::as_str)
            .is_some(),
        "the descendant should have served the protocol at all; it reported:\n{reported}"
    );
    // Association arrived by inheritance too: the server reports the namespace it
    // bound to, which came from the same stamp as the state root.
    let instructions = initialized
        .pointer("/result/instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        instructions.contains("namespace 'alpha'"),
        "the descendant should have associated with the bundle it was stamped \
         into; it reported instructions {instructions:?} in:\n{reported}"
    );

    // The bundle came back hosted, which is only true of a relay that is actually
    // serving it. Deliberately not asserted on `isError`, which is false either
    // way: an unreachable relay still yields a successful tool call reporting the
    // bundle down, so that field cannot tell the two apart.
    let listed = listed_bundle(
        responses
            .get(&2)
            .expect("awaited response with id 2 is present"),
        "alpha",
    );
    assert_eq!(
        listed.get("hosted").and_then(Value::as_bool),
        Some(true),
        "the descendant should have found the bundle hosted on the relay that \
         spawned it; it reported bundle {listed} in:\n{reported}"
    );
}

#[test]
fn a_relay_spawned_member_client_reaches_the_spawning_relay() {
    assert_rendezvous_survives_declaration(MEMBER_DECLARED_STATE_ROOT);
}

// The blank case run for real rather than by argv alone. Blank reads as absent
// in every other tier, so an upsert-if-absent implementation passes the
// conflicting case above and fails here — and it fails as a child that cannot
// find its relay, which is the consequence that matters.
#[test]
fn a_blank_declaration_still_leaves_the_member_able_to_reach_the_relay() {
    assert_rendezvous_survives_declaration(MEMBER_BLANK_STATE_ROOT);
}
