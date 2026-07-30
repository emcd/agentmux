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
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

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

/// Waits for the spawned member's client to finish its second exchange.
///
/// Waiting on a non-empty file is not enough: the `initialize` response arrives
/// first, and returning on it tears the relay down before the `tools/call` the
/// assertions read. The `tools/call` carries `id` 2, and it arrives either way —
/// an unreachable relay yields a successful call reporting the bundle down — so
/// this is a wait for the exchange to finish rather than for it to succeed.
fn await_rendezvous_report(report: &std::path::Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_seen = String::new();
    while Instant::now() < deadline {
        if let Ok(contents) = fs::read_to_string(report) {
            if contents.contains(r#""id":2"#) {
                return contents;
            }
            last_seen = contents;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "the spawned member's agentmux client never answered the tools/call at {}; \
         it reported:\n{last_seen}",
        report.display()
    );
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
    let host_child = process::strip_bring_up_context_std(&mut relay_command)
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
        .expect("spawn agentmux host relay");
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

    let reported = await_rendezvous_report(&report);

    shutdown_relay_if_present(&state_root, "alpha");
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();

    assert!(
        reported.contains("\"protocolVersion\""),
        "the descendant should have served the protocol at all; it reported:\n{reported}"
    );
    assert!(
        !reported.contains("relay_unavailable")
            && !reported.contains("relay socket is not present"),
        "the spawned member's host mcp descendant must have reached the spawning \
         relay, not a default root; it reported:\n{reported}"
    );
    // Association arrived by inheritance too: the server reports the namespace it
    // bound to, which came from the same stamp as the state root.
    assert!(
        reported.contains("namespace 'alpha'"),
        "the descendant should have associated with the bundle it was stamped \
         into; it reported:\n{reported}"
    );
    // The bundle came back hosted, which is only true of a relay that is actually
    // serving it. Deliberately not asserted on `isError`, which is false either
    // way: an unreachable relay still yields a successful tool call reporting the
    // bundle down, so that field cannot tell the two apart. The payload is nested
    // JSON, hence the escaped quotes.
    assert!(
        reported.contains(r#"\"hosted\":true"#),
        "the descendant should have found the bundle hosted on the relay that \
         spawned it; it reported:\n{reported}"
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

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("spawn agentmux host relay --no-autostart");
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
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
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

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("spawn agentmux host relay --no-autostart");
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
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
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

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("spawn agentmux host relay --no-autostart");
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
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
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

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("spawn agentmux host relay --no-autostart");
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
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
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

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("spawn agentmux host relay --no-autostart");
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

    shutdown_relay_if_present(&state_root, "alpha");
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
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

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("spawn agentmux host relay --no-autostart");
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
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
}
