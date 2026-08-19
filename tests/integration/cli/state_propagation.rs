//! What a spawned member actually receives from the relay that started it.
//!
//! Two levels of evidence, deliberately both. Most tests here drive
//! `agentmux up` against a fake tmux and assert on the argument vector tmux was
//! handed, which pins the stamp precisely and runs in milliseconds.
//! `a_relay_spawned_member_client_reaches_the_spawning_relay` instead uses real
//! tmux and a real `agentmux host mcp` descendant, because argv shows a value
//! being passed while the defect being guarded against is a child *resolving*
//! somewhere the relay never bound. Only a live child arriving at the right
//! socket rules that out.

use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;

use super::super::support::process;
use super::helpers::*;

/// A state root the member declares for itself, which the relay must overwrite.
const MEMBER_DECLARED_STATE_ROOT: &str = "/nowhere/member-declared";

/// A blank declaration, which must be overwritten rather than suppressing the
/// stamp. Blank reads as absent everywhere else, so this is the case where an
/// upsert-if-absent implementation would look correct and still break the
/// rendezvous.
const MEMBER_BLANK_STATE_ROOT: &str = "";

/// Appends a session-level `AGENTMUX_STATE_DIRECTORY` to a written bundle file,
/// so the spawn has an operator-declared value to contend with.
fn declare_member_state_directory(config_root: &std::path::Path, bundle_name: &str, value: &str) {
    let path = config_root
        .join("bundles")
        .join(format!("{bundle_name}.toml"));
    let mut bundle = fs::read_to_string(&path).expect("read bundle configuration");
    bundle.push_str(&format!(
        "\n[[sessions.environment]]\nname = \"AGENTMUX_STATE_DIRECTORY\"\nvalue = \"{value}\"\n"
    ));
    fs::write(&path, bundle).expect("write bundle configuration");
}

/// Returns the single recorded `new-session` invocation.
fn recorded_new_session(log: &str) -> &str {
    let mut lines = log
        .lines()
        .filter(|line| line.contains("new-session"))
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one new-session invocation, got:\n{log}"
    );
    lines.pop().expect("one new-session line")
}

/// Writes a bundle whose member, once spawned, runs an agentmux client that
/// must find the relay on its own.
///
/// The client is given a configuration directory but deliberately **no**
/// `--state-directory`, so the only way it can reach the relay is by resolving
/// `AGENTMUX_STATE_DIRECTORY` out of the environment it inherited. Its output is
/// captured to `report`.
fn write_rendezvous_bundle(
    config_root: &std::path::Path,
    report: &std::path::Path,
    member_directory: &std::path::Path,
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
fn await_rendezvous_report(report: &std::path::Path) -> (HashMap<u64, Value>, String) {
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
    assert!(
        up.status.success(),
        "up should start the member; stderr:\n{}",
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

/// Brings a relay up with `declared` as the member's own
/// `AGENTMUX_STATE_DIRECTORY` and returns the recorded `new-session`
/// invocation together with the relay's state root.
fn spawn_with_declared_state_root(
    temporary: &TempDir,
    declared: &str,
) -> (String, std::path::PathBuf) {
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
    declare_member_state_directory(&config_root, "alpha", declared);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--no-autostart",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
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
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "up should succeed; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    let new_session = recorded_new_session(&log).to_string();

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
    (new_session, state_root)
}

#[test]
fn a_blank_member_declaration_does_not_suppress_the_stamp() {
    let temporary = TempDir::new().expect("temporary");
    let (new_session, state_root) =
        spawn_with_declared_state_root(&temporary, MEMBER_BLANK_STATE_ROOT);

    assert!(
        new_session.contains(&format!(
            "-e AGENTMUX_STATE_DIRECTORY={}",
            state_root.display()
        )),
        "a blank declaration must be overwritten, not treated as absent-and-left; got:\n\
         {new_session}"
    );
    assert!(
        !new_session.contains("-e AGENTMUX_STATE_DIRECTORY "),
        "no blank value may survive into the spawn; got:\n{new_session}"
    );
}

#[test]
fn a_spawned_member_receives_the_relays_state_root_over_its_own_declaration() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
    declare_member_state_directory(&config_root, "alpha", MEMBER_DECLARED_STATE_ROOT);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--no-autostart",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
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
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux up");
    assert!(up.status.success(), "up should succeed");

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    let new_session = recorded_new_session(&log);

    assert!(
        new_session.contains(&format!(
            "-e AGENTMUX_STATE_DIRECTORY={}",
            state_root.display()
        )),
        "the spawn must carry the relay's state root; got:\n{new_session}"
    );
    assert!(
        !new_session.contains(MEMBER_DECLARED_STATE_ROOT),
        "a member-declared state root must be overwritten, not preserved; got:\n{new_session}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

/// Builds a state root deep enough that `<state_root>/bundles/alpha/tmux.sock`
/// overshoots `sun_path`, rather than merely approaching it — a fixture near
/// the boundary passes whether or not the fix is present.
///
/// Gated with its caller: on a non-Linux target the deep-root bring-up is not
/// expected to succeed and the test is absent, which would leave this dead under
/// `clippy --all-targets -D warnings`.
#[cfg(target_os = "linux")]
fn deep_state_root(base: &std::path::Path) -> std::path::PathBuf {
    // The crate's own constant rather than a literal: the limit is 107 on Linux
    // and 103 on Darwin, and a fixture hardcoding one of them would overshoot by
    // a different margin on the other.
    use agentmux::runtime::sockets::UNIX_SOCKET_PATH_MAXIMUM;

    /// Clears the limit by a wide margin instead of sitting on it.
    const OVERSHOOT: usize = 60;

    let mut root = base.to_path_buf();
    while root.join("bundles/alpha/tmux.sock").as_os_str().len()
        <= UNIX_SOCKET_PATH_MAXIMUM + OVERSHOOT
    {
        root = root.join("deeply-nested-state-directory");
    }
    assert!(
        root.join("relay.sock").as_os_str().len() > UNIX_SOCKET_PATH_MAXIMUM,
        "the fixture must overshoot the limit for the relay socket too"
    );
    root
}

// Linux only, and that is the behavior rather than a test-environment excuse.
// Shortening the address depends on `/proc/self/fd`, so on Darwin the full path
// is used and a root this deep is genuinely unreachable. The non-Linux
// expectation — a structured refusal naming the limit — is asserted directly
// against `runtime::sockets` in `tests/unit/runtime_sockets.rs`, which is where
// it can be stated without standing up a relay that cannot come up.
#[cfg(target_os = "linux")]
#[test]
fn a_relay_comes_up_under_a_state_root_longer_than_sun_path() {
    // Normalizing the state root to an absolute path removed the relative-path
    // escape hatch deep hierarchies relied on, so binding has to stop scaling
    // with depth. Driving a real relay bring-up is the only way to see that:
    // the relay binds its socket, publishes the ready sentinel, and answers a
    // client that connects to the same long path.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = deep_state_root(temporary.path());
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--no-autostart",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    // Reaching the relay is the assertion: `up` is a client that connects to
    // the same over-long socket path the relay bound.
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
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "up should reach a relay under a deep state root; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    let new_session = recorded_new_session(&log);
    assert!(
        new_session.starts_with("-S tmux.sock "),
        "tmux must still be addressed by the bare socket name; got:\n{new_session}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn a_relative_tmux_wrapper_still_resolves_against_the_launch_directory() {
    // Running the client from the socket's directory changed what a relative
    // program path means: the kernel resolves a value containing a separator
    // against the *child's* working directory, so `./fake-tmux.sh` would be
    // looked for under the bundle runtime directory. Every other test here
    // passes an absolute wrapper and cannot see it.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let wrapper_directory = temporary.path().join("wrappers");
    fs::create_dir_all(&wrapper_directory).expect("create wrapper directory");
    let fake_tmux = wrapper_directory.join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    // Relative, with a separator, interpreted against the relay's own working
    // directory — which is the wrapper's parent, not the bundle runtime.
    let relative_wrapper = std::path::PathBuf::from("wrappers/fake-tmux.sh");

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .current_dir(temporary.path())
            .args([
                "host",
                "relay",
                "--no-autostart",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &relative_wrapper)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(temporary.path())
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
        .env("AGENTMUX_TMUX_COMMAND", &relative_wrapper)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "a relative wrapper must still be found; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux))
        .expect("the relative wrapper must have run and recorded its invocations");
    assert!(
        log.contains("new-session"),
        "the wrapper must have been reached for session creation; got:\n{log}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn a_bare_tmux_command_resolves_through_the_launch_directorys_path() {
    // The companion to the relative-wrapper case, for the other kind of relative
    // reference. A bare name carries no separator and goes through `PATH`, but a
    // `PATH` *entry* may itself be relative, so it moves with the working
    // directory just as a `./wrapper.sh` would. Configuring an absolute wrapper —
    // what every other test here does — cannot see it.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let wrapper_directory = temporary.path().join("wrappers");
    fs::create_dir_all(&wrapper_directory).expect("create wrapper directory");
    let fake_tmux = wrapper_directory.join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    // A relative first entry, resolved against the relay's own working directory.
    // The rest of the inherited `PATH` is kept because the wrapper's `env`
    // shebang needs it.
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let search_path = format!("wrappers:{inherited_path}");

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .current_dir(temporary.path())
            .args([
                "host",
                "relay",
                "--no-autostart",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", "fake-tmux.sh")
            .env("PATH", &search_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(temporary.path())
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
        .env("AGENTMUX_TMUX_COMMAND", "fake-tmux.sh")
        .env("PATH", &search_path)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "a bare command on a relative PATH entry must still be found; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux))
        .expect("the wrapper on the relative PATH entry must have run and recorded");
    assert!(
        log.contains("new-session"),
        "the wrapper must have been reached for session creation; got:\n{log}"
    );

    // The other half, and the reason the fix resolves the program rather than
    // normalizing the environment: the client's `PATH` must reach it untouched.
    // A tmux client hands its environment to a server it starts and thence to
    // every pane, and a pane is started with `-c <member directory>`, so
    // rewriting a relative entry here would silently repoint coder lookups from
    // the member's directory to the relay's. The fake tmux stands where the
    // client stands, so what it recorded is what a pane would inherit.
    let observed_path = fs::read_to_string(fake_tmux_search_path_file(&fake_tmux))
        .expect("the wrapper must have recorded the PATH it inherited");
    assert_eq!(
        observed_path.trim(),
        search_path,
        "the client's PATH must arrive exactly as inherited, relative entry included"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn a_bare_tmux_command_keeps_execvp_search_order_across_relative_entries() {
    // The lookup for a bare name belongs to execvp, and its search is more than
    // "first file of that name": a non-executable candidate is passed over for a
    // later entry. This drives two relative entries where the first shadows the
    // second by name only, so a hand-rolled lookup that stopped at the first
    // match would run nothing.
    //
    // It does not reproduce the sharper case of a file carrying an execute bit
    // the effective user cannot use — that needs a file owned by another
    // principal, which an unprivileged fixture cannot create. An execute-only
    // script does not stand in for it either: exec of a shebang script succeeds
    // and the interpreter fails afterwards, so the search is never resumed. What
    // rules that case out is not testing it but declining to reimplement the
    // search at all.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let shadowed_directory = temporary.path().join("wrappers-shadowed");
    let wrapper_directory = temporary.path().join("wrappers");
    fs::create_dir_all(&shadowed_directory).expect("create shadowed directory");
    fs::create_dir_all(&wrapper_directory).expect("create wrapper directory");
    // Same name, earlier on PATH, and not executable at all.
    let shadowed = shadowed_directory.join("fake-tmux.sh");
    fs::write(&shadowed, "#!/usr/bin/env bash\nexit 3\n").expect("write shadowed wrapper");
    fs::set_permissions(&shadowed, fs::Permissions::from_mode(0o644))
        .expect("make shadowed wrapper non-executable");
    let fake_tmux = wrapper_directory.join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let search_path = format!("wrappers-shadowed:wrappers:{inherited_path}");

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .current_dir(temporary.path())
            .args([
                "host",
                "relay",
                "--no-autostart",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", "fake-tmux.sh")
            .env("PATH", &search_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(temporary.path())
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
        .env("AGENTMUX_TMUX_COMMAND", "fake-tmux.sh")
        .env("PATH", &search_path)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "the executable later entry must be reached; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux))
        .expect("the executable wrapper must have run and recorded");
    assert!(
        log.contains("new-session"),
        "the wrapper on the later PATH entry must have been reached; got:\n{log}"
    );
    assert!(
        !fake_tmux_log_path(&shadowed).exists(),
        "the shadowing non-executable entry must not have run"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn tmux_is_addressed_relative_to_its_own_socket_directory() {
    // The socket address must not scale with state-root depth: tmux binds the
    // `-S` path itself, and `<state_root>/bundles/<bundle>/tmux.sock` is the
    // longest path this project constructs. Asserting on the recorded
    // invocation is what makes this a claim about what tmux received rather
    // than about an intermediate value.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--no-autostart",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
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
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux up");
    assert!(up.status.success(), "up should succeed");

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    for invocation in log.lines().filter(|line| !line.trim().is_empty()) {
        assert!(
            invocation.starts_with("-S tmux.sock "),
            "every tmux invocation must address the bare socket name; got:\n{invocation}"
        );
    }

    // And the start directory stays explicit, because an omitted `-c` would
    // take the client's working directory — now the bundle runtime directory
    // rather than wherever the relay was launched.
    let new_session = recorded_new_session(&log);
    assert!(
        new_session.contains("-c /tmp"),
        "new-session must pass the member's declared directory; got:\n{new_session}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}
