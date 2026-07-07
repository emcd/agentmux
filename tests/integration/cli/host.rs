use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use agentmux::runtime::paths::RelayRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::super::support::process;
use super::helpers::*;

/// Wait budget for observing a watcher-driven signal: a reload/suppression
/// inscription, an eviction frame on a live stream, or a catalog change probed
/// via Hello. Generous because the pre-commit hook runs the full suite in
/// parallel on arbitrarily loaded machines; every wait returns as soon as the
/// signal arrives, so the budget is only paid on genuine failure.
const WATCHER_SIGNAL_WAIT_BUDGET: Duration = Duration::from_secs(30);

/// Negative-assertion budget for `--no-watch` and `watch-bundles = false`
/// tests: how long to wait after writing a runtime bundle before asserting
/// the relay still reports it as unknown. A watcher (if one existed)
/// would have had this long to reconcile. The watcher polls on the
/// bundle-watcher's internal interval (well under 1 second even on
/// slow CI); 1 second is generous margin to ensure a missing watcher
/// is correctly distinguished from a slow watcher.
const WATCHER_RECONCILE_BUDGET: Duration = Duration::from_secs(1);

/// Connects a raw client to the live relay socket, sends one Hello, and returns
/// the first server frame (a `hello_ack` on acceptance or an error `response`
/// on rejection). Used to exercise the relay-wide credential-enforcement flag
/// end-to-end through the hosted binary.
fn relay_hello_first_frame(state_root: &Path, principal_id: &str, identity_token: &str) -> Value {
    let socket = RelayRuntimePaths::resolve(state_root).relay_socket;
    let mut stream = UnixStream::connect(&socket).expect("connect relay socket");
    let hello = json!({
        "frame": "hello",
        "schema_version": "1",
        "principal_id": principal_id,
        "identity_token": identity_token,
    });
    let encoded = serde_json::to_string(&hello).expect("encode hello");
    stream
        .write_all(format!("{encoded}\n").as_bytes())
        .expect("write hello");
    stream.flush().expect("flush hello");
    let reader_stream = stream.try_clone().expect("clone relay stream");
    reader_stream
        .set_read_timeout(Some(WATCHER_SIGNAL_WAIT_BUDGET))
        .expect("set hello read timeout");
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read relay frame");
    serde_json::from_str(line.trim_end()).expect("decode relay frame")
}

fn write_bundle_configuration_with_tmux_and_acp_failure(config_root: &Path, bundle_name: &str) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "tmux-default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"

[[coders]]
id = "acp-broken"

[coders.acp]
channel = "stdio"
command = "/definitely/missing/agentmux-acp"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
# The CLI lists this bundle as the relay-wide `user@GLOBAL` operator, whose home
# namespace is GLOBAL; reaching into a bundle is cross-namespace and requires
# all.
list = "all"
look = "self"
send = "home"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        r#"
format-version = 1
autostart = true
groups = ["dev"]

[[sessions]]
id = "alpha"
name = "alpha"
directory = "/tmp"
coder = "tmux-default"

[[sessions]]
id = "bravo"
name = "bravo"
directory = "/tmp"
coder = "acp-broken"
"#,
    )
    .expect("write bundle config");
}

/// Writes a bundle whose only session is an ACP member with a missing binary,
/// so every configured session fails to start and the autostart summary reports
/// the bundle as `failed`.
fn write_all_acp_failure_bundle(config_root: &Path, bundle_name: &str) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "acp-broken"

[coders.acp]
channel = "stdio"
command = "/definitely/missing/agentmux-acp"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all"
look = "self"
send = "home"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        r#"
format-version = 1
autostart = true
groups = ["dev"]

[[sessions]]
id = "bravo"
name = "bravo"
directory = "/tmp"
coder = "acp-broken"
"#,
    )
    .expect("write bundle config");
}

#[test]
fn host_relay_rejects_positional_bundle_selector() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "alpha"])
        .output()
        .expect("run agentmux host relay");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_arguments"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_rejects_group_selector_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--group", "dev"])
        .output()
        .expect("run agentmux host relay with group selector");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--group") && stderr.contains("unknown argument"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_default_mode_starts_autostart_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(false),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");
    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");

    assert!(output.status.success(), "command should succeed");
    let summary_json = parse_summary_json_line(&output.stdout);
    let bundles = summary_json["bundles"]
        .as_array()
        .expect("startup summary bundles");
    let alpha = bundles
        .iter()
        .find(|bundle| bundle["bundle_name"] == "alpha")
        .expect("alpha startup summary");
    let bravo = bundles
        .iter()
        .find(|bundle| bundle["bundle_name"] == "bravo")
        .expect("bravo startup summary");
    assert!(
        summary_json["host_mode"] == "autostart",
        "unexpected summary: {summary_json}"
    );
    assert!(
        summary_json["hosted_bundle_count"] == 1
            && summary_json["skipped_bundle_count"] == 1
            && summary_json["failed_bundle_count"] == 0
            && summary_json["hosted_any"] == true,
        "unexpected summary: {summary_json}"
    );
    assert_eq!(alpha["outcome"], "hosted");
    assert_eq!(bravo["outcome"], "skipped");
    assert_eq!(bravo["reason_code"], "process_only");
}

#[test]
fn host_relay_records_startup_failures_and_list_reports_degraded_health() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_tmux_and_acp_failure(&config_root, "alpha");
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");

    let listed = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "principals",
            "--namespace",
            "alpha",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run list sessions");
    assert!(listed.status.success(), "list sessions should succeed");
    let listed_json: Value = serde_json::from_slice(&listed.stdout).expect("decode list payload");
    assert_eq!(listed_json["bundle"]["state"], "up");
    assert_eq!(listed_json["bundle"]["startup_health"], "degraded");
    let startup_failure_count = listed_json["bundle"]["startup_failure_count"]
        .as_u64()
        .expect("startup failure count");
    assert!(
        startup_failure_count >= 1,
        "expected startup failure record in list payload: {listed_json}"
    );
    let failures = listed_json["bundle"]["recent_startup_failures"]
        .as_array()
        .expect("startup failures array");
    let bravo_failure = failures
        .iter()
        .find(|entry| entry["session_id"] == "bravo")
        .unwrap_or_else(|| panic!("expected ACP startup failure for bravo session: {listed_json}"));
    // The record must carry the true bootstrap cause plumbed out of the worker
    // task — here the ACP child binary could not be spawned — rather than the
    // generic "worker unavailable" placeholder the startup poller sees from the
    // readiness state alone.
    assert_eq!(
        bravo_failure["code"], "runtime_startup_failed",
        "bravo startup failure should carry the bootstrap failure code, not the \
         generic unavailable placeholder: {listed_json}"
    );
    assert!(
        bravo_failure["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("spawn ACP stdio command failed")),
        "bravo startup failure should surface the true spawn cause: {listed_json}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
}

#[test]
fn host_relay_autostart_summary_folds_failed_session_reasons() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_all_acp_failure_bundle(&config_root, "alpha");
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");
    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    let summary_json = parse_summary_json_line(&output.stdout);
    let alpha = summary_json["bundles"]
        .as_array()
        .expect("startup summary bundles")
        .iter()
        .find(|bundle| bundle["bundle_name"] == "alpha")
        .expect("alpha startup summary");
    assert_eq!(alpha["outcome"], "failed");
    // The blanket "zero configured sessions reached ready state" placeholder is
    // replaced with the real per-session cause folded from the startup report.
    let reason = alpha["reason"].as_str().expect("failed reason string");
    assert!(
        reason.contains("bravo") && reason.contains("spawn ACP stdio command failed"),
        "summary reason should name the failed session and its cause: {summary_json}"
    );
    let failed_sessions = alpha["details"]["failed_sessions"]
        .as_array()
        .expect("failed_sessions details array");
    assert!(
        failed_sessions.iter().any(|entry| {
            entry["session_id"] == "bravo"
                && entry["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("spawn ACP stdio command failed"))
        }),
        "failed_sessions should carry the structured bravo cause: {summary_json}"
    );
}

#[test]
fn host_relay_clears_startup_failures_for_sessions_that_start_successfully() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["primary"], Some(true));
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let bundle_runtime = state_root.join("bundles").join("alpha");
    fs::create_dir_all(&bundle_runtime).expect("create bundle runtime directory");
    fs::write(
        bundle_runtime.join("startup_failures.json"),
        r#"{
            "schema_version": 1,
            "next_sequence": 3,
            "records": [
                {
                    "bundle_name": "alpha",
                    "session_id": "primary",
                    "transport": "tmux",
                    "code": "runtime_startup_failed",
                    "reason": "stale failure from prior run",
                    "timestamp": "2026-05-01T00:00:00Z",
                    "sequence": 1
                },
                {
                    "bundle_name": "alpha",
                    "session_id": "ghost",
                    "transport": "tmux",
                    "code": "runtime_startup_failed",
                    "reason": "unrelated failure that must be preserved",
                    "timestamp": "2026-05-01T00:00:01Z",
                    "sequence": 2
                }
            ]
        }"#,
    )
    .expect("seed startup_failures.json");

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");

    let listed = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "principals",
            "--namespace",
            "alpha",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run list sessions");
    assert!(listed.status.success(), "list sessions should succeed");
    let listed_json: Value = serde_json::from_slice(&listed.stdout).expect("decode list payload");
    let failures = listed_json["bundle"]["recent_startup_failures"]
        .as_array()
        .expect("startup failures array");
    assert!(
        failures
            .iter()
            .all(|entry| entry["session_id"] != "primary"),
        "expected primary startup failure to be cleared after successful start: {listed_json}"
    );
    assert!(
        failures.iter().any(|entry| entry["session_id"] == "ghost"),
        "expected unrelated ghost startup failure to be preserved: {listed_json}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
}

fn write_bundle_configuration_with_invalid_policy_scope(config_root: &Path, bundle_name: &str) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "tmux-default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"
choose = "everywhere"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        r#"
format-version = 1
autostart = true
groups = ["dev"]

[[sessions]]
id = "alpha"
name = "alpha"
directory = "/tmp"
coder = "tmux-default"
"#,
    )
    .expect("write bundle config");
}

// Replays the 2026-06-11 outage shape: every autostart bundle fails (here from
// a policy validation rejection) and the host exits. The journal (stderr) and
// the inscription log must each carry a per-bundle reason with the structured
// error details, not just the aggregate bundle count.
#[test]
fn host_relay_startup_failure_emits_per_bundle_reason_with_details() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_invalid_policy_scope(&config_root, "alpha");

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux host relay");
    assert!(
        !output.status.success(),
        "host relay should exit nonzero when every bundle fails to start"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bundle 'alpha' failed to start (validation_invalid_policy_scope)"),
        "expected per-bundle failure reason on stderr: {stderr}"
    );
    assert!(
        stderr.contains("\"control\":\"choose\"") && stderr.contains("\"value\":\"everywhere\""),
        "expected structured details on stderr: {stderr}"
    );

    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let startup_failed = inscriptions
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("decode inscription line"))
        .find(|entry| entry["event"] == "relay.bundle.startup_failed")
        .expect("relay.bundle.startup_failed inscription should be emitted");
    assert_eq!(startup_failed["details"]["bundle_name"], "alpha");
    assert_eq!(
        startup_failed["details"]["reason_code"],
        "validation_invalid_policy_scope"
    );
    assert_eq!(startup_failed["details"]["details"]["control"], "choose");
    assert_eq!(startup_failed["details"]["details"]["value"], "everywhere");
}

#[test]
fn host_relay_no_autostart_mode_reports_process_only_summary() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
            "--config-directory",
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
    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay --no-autostart");

    assert!(output.status.success(), "command should succeed");
    let summary_json = parse_summary_json_line(&output.stdout);
    let bundles = summary_json["bundles"]
        .as_array()
        .expect("startup summary bundles");
    let alpha = bundles
        .iter()
        .find(|bundle| bundle["bundle_name"] == "alpha")
        .expect("alpha startup summary");
    assert!(
        summary_json["host_mode"] == "process_only",
        "unexpected summary: {summary_json}"
    );
    assert!(
        summary_json["hosted_bundle_count"] == 0
            && summary_json["skipped_bundle_count"] == 1
            && summary_json["failed_bundle_count"] == 0
            && summary_json["hosted_any"] == false,
        "unexpected summary: {summary_json}"
    );
    assert_eq!(alpha["outcome"], "skipped");
    assert_eq!(alpha["reason_code"], "process_only");
}

// `--require-credentials` threads from argument parsing through `serve_relay_host`
// and the accept loop into Hello credential verification: with the flag set, a
// session principal presenting the `socket-trust` sentinel is rejected.
#[test]
fn host_relay_require_credentials_flag_rejects_socket_trust() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(true));
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
            "--require-credentials",
            "--config-directory",
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
        .expect("spawn agentmux host relay --require-credentials");
    wait_for_relay_ready(&state_root, "alpha");

    let frame = relay_hello_first_frame(&state_root, "a@alpha", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(frame["response"]["kind"], "error");
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_credential_required"
    );
}

// Contrast for the flag above: without `--require-credentials`, the same
// `socket-trust` Hello is accepted, confirming the flag (not an always-on
// default) is what drives rejection.
#[test]
fn host_relay_without_require_credentials_accepts_socket_trust() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(true));
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");

    let frame = relay_hello_first_frame(&state_root, "a@alpha", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(frame["frame"], "hello_ack", "expected hello_ack: {frame:?}");
    assert_eq!(frame["principal_id"], "a@alpha");
}

// relay.toml `require-session-credentials = true` drives enforcement without the
// CLI flag: the resolved configuration threads from `relay.toml` through startup
// into Hello verification, so a `socket-trust` session is rejected.
#[test]
fn host_relay_require_credentials_from_relay_toml_rejects_socket_trust() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(true));
    fs::write(
        config_root.join("relay.toml"),
        "require-session-credentials = true\n",
    )
    .expect("write relay.toml");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
            "--config-directory",
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
        .expect("spawn agentmux host relay with relay.toml require-session-credentials");
    wait_for_relay_ready(&state_root, "alpha");

    let frame = relay_hello_first_frame(&state_root, "a@alpha", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_credential_required"
    );
}

/// Opens a persistent stream connection, sends one Hello, and returns the live
/// stream, a buffered reader (with a read timeout so a missing frame fails the
/// test rather than hanging), and the first server frame. Held open so a
/// later watcher-driven eviction frame can be observed on the same connection.
fn relay_hello_keepalive(
    state_root: &Path,
    principal_id: &str,
    identity_token: &str,
) -> (UnixStream, BufReader<UnixStream>, Value) {
    let socket = RelayRuntimePaths::resolve(state_root).relay_socket;
    let mut stream = UnixStream::connect(&socket).expect("connect relay socket");
    let reader_stream = stream.try_clone().expect("clone relay stream");
    reader_stream
        .set_read_timeout(Some(WATCHER_SIGNAL_WAIT_BUDGET))
        .expect("set relay read timeout");
    let hello = json!({
        "frame": "hello",
        "schema_version": "1",
        "principal_id": principal_id,
        "identity_token": identity_token,
    });
    let encoded = serde_json::to_string(&hello).expect("encode hello");
    stream
        .write_all(format!("{encoded}\n").as_bytes())
        .expect("write hello");
    stream.flush().expect("flush hello");
    let mut reader = BufReader::new(reader_stream);
    let frame = read_next_frame(&mut reader).expect("hello frame");
    (stream, reader, frame)
}

/// Reads the next newline-delimited frame, or `None` on EOF or read timeout.
fn read_next_frame(reader: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => Some(serde_json::from_str(line.trim_end()).expect("decode relay frame")),
        Err(_) => None,
    }
}

/// Asserts the relay armed its bundle watcher before publishing readiness.
/// `relay.bundle.watch.started` is inscribed before the ready sentinel, so
/// after `wait_for_relay_ready` its absence means the watch could not be
/// created (for example inotify instance exhaustion) and the relay is serving
/// without reconciliation — every watcher signal the test waits on afterwards
/// would starve for the full budget with no explanation.
fn assert_bundle_watch_started(inscriptions_root: &Path) {
    let log = fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default();
    let started = log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| entry["event"] == "relay.bundle.watch.started");
    assert!(
        started,
        "relay published readiness without arming the bundle watcher; relay inscriptions: {log}"
    );
}

/// Unwraps a watcher-driven signal, panicking with the relay inscriptions log
/// so a starved wait reports how far the watcher actually got (event observed?
/// unload/reload inscribed?) instead of a bare expect message.
fn expect_watcher_signal<T>(signal: Option<T>, what: &str, inscriptions_root: &Path) -> T {
    signal.unwrap_or_else(|| {
        panic!(
            "{what} not observed within {WATCHER_SIGNAL_WAIT_BUDGET:?}; relay inscriptions: {}",
            fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
        )
    })
}

/// Repeatedly opens a fresh Hello connection until `accept` matches the returned
/// frame or the deadline passes. Used to observe the watcher's eventual catalog
/// state (a newly loaded bundle accepting Hello, or an unloaded one rejecting).
fn poll_hello_first_frame(
    state_root: &Path,
    principal_id: &str,
    identity_token: &str,
    accept: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + WATCHER_SIGNAL_WAIT_BUDGET;
    loop {
        let frame = relay_hello_first_frame(state_root, principal_id, identity_token);
        if accept(&frame) || Instant::now() >= deadline {
            return frame;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

// A bundle TOML file added at runtime is picked up by the watcher: the relay
// loads and starts the bundle without a restart, and a new connection to that
// bundle succeeds where it was previously rejected as an unknown bundle.
#[test]
fn host_relay_watcher_loads_new_bundle_file_at_runtime() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // The bravo bundle does not exist yet: Hello is rejected as unknown.
    let before = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");
    // Add the bravo bundle file at runtime; the watcher should load it.
    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    let added = poll_hello_first_frame(&state_root, "b@bravo", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        before["frame"], "response",
        "expected error frame: {before:?}"
    );
    assert_eq!(
        before["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
    assert_eq!(
        added["frame"],
        "hello_ack",
        "expected hello_ack after add: {added:?}; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(added["principal_id"], "b@bravo");
}

// A bundle TOML file added at runtime with `autostart = false` is loaded held:
// the relay learns it (its members register as offline shells, so Hello is
// accepted) but does not start it, mirroring the boot-time process-only path.
#[test]
fn host_relay_watcher_loads_non_autostart_bundle_held_without_starting() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // Add a non-autostart bravo bundle at runtime; the watcher should register it
    // held rather than start it.
    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(false),
    );
    let loaded_held =
        poll_inscription_event(&inscriptions_root, "relay.bundle.loaded_held", "bravo");
    // The held bundle is nonetheless known: Hello to one of its members succeeds.
    let known = poll_hello_first_frame(&state_root, "b@bravo", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        loaded_held,
        "expected relay.bundle.loaded_held for the non-autostart runtime add; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(
        known["frame"], "hello_ack",
        "held bundle should still be known: {known:?}"
    );
    // It must not have been started: no `relay.bundle.loaded` for bravo.
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let started = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.loaded" && entry["details"]["bundle_name"] == "bravo"
        });
    assert!(
        !started,
        "non-autostart bundle must not be started on runtime add: {inscriptions}"
    );
}

// A bundle TOML file removed at runtime unloads the bundle: active sessions
// receive a `runtime_bundle_unloaded` error frame before disconnect, and
// subsequent connection attempts to that bundle are rejected as unknown.
#[test]
fn host_relay_watcher_unloads_removed_bundle_file_at_runtime() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    let (_stream, mut reader, hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    fs::remove_file(config_root.join("bundles").join("alpha.toml")).expect("remove bundle file");
    // The active session's connection receives the typed unload frame.
    let eviction = expect_watcher_signal(
        read_next_frame(&mut reader),
        "unload eviction frame",
        &inscriptions_root,
    );
    // The catalog is updated before the eviction frame is written, so a new
    // Hello to the unloaded bundle is now rejected as unknown.
    let after = relay_hello_first_frame(&state_root, "a@alpha", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(hello["frame"], "hello_ack", "expected hello_ack: {hello:?}");
    assert_eq!(
        eviction["frame"], "response",
        "expected eviction frame: {eviction:?}"
    );
    assert_eq!(
        eviction["response"]["error"]["code"],
        "runtime_bundle_unloaded"
    );
    assert_eq!(
        after["frame"], "response",
        "expected unknown-bundle frame: {after:?}"
    );
    assert_eq!(
        after["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}

// A bundle TOML file modified at runtime is treated as a full teardown +
// reload: active sessions receive a `runtime_bundle_reloaded` error frame before
// disconnect, and the relay reloads the bundle (a fresh connection succeeds).
#[test]
fn host_relay_watcher_reloads_modified_bundle_file_at_runtime() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    let (_stream, mut reader, hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    // Modify the alpha bundle file (add a session): the watcher reloads it.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(true),
    );
    let eviction = expect_watcher_signal(
        read_next_frame(&mut reader),
        "reload eviction frame",
        &inscriptions_root,
    );
    // After reload the bundle is still loaded: a fresh Hello succeeds.
    let after = poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(hello["frame"], "hello_ack", "expected hello_ack: {hello:?}");
    assert_eq!(
        eviction["frame"], "response",
        "expected eviction frame: {eviction:?}"
    );
    assert_eq!(
        eviction["response"]["error"]["code"],
        "runtime_bundle_reloaded"
    );
    assert_eq!(
        after["frame"],
        "hello_ack",
        "expected hello_ack after reload: {after:?}; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(after["principal_id"], "a@alpha");
}

/// Polls the relay inscriptions log until an entry with `event` naming
/// `bundle_name` appears, returning `true` on a match or `false` once the
/// deadline passes. Used to observe a watcher outcome that emits no stream frame
/// (so it cannot be awaited on a keepalive connection).
fn poll_inscription_event(inscriptions_root: &Path, event: &str, bundle_name: &str) -> bool {
    let log = inscriptions_root.join("relay.log");
    let deadline = Instant::now() + WATCHER_SIGNAL_WAIT_BUDGET;
    loop {
        if let Ok(contents) = fs::read_to_string(&log) {
            let found = contents
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|entry| {
                    entry["event"] == event && entry["details"]["bundle_name"] == bundle_name
                });
            if found {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

// A bundle the operator explicitly took down via `down` is not silently brought
// back up when its configuration file is edited: the watcher absorbs the new
// fingerprint and records `relay.bundle.reload_suppressed_downed` instead of
// reloading (and restarting) the runtime.
#[test]
fn host_relay_watcher_preserves_down_intent_across_file_edit() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // Operator takes the bundle down. This flows over the live relay socket and
    // records the down intent on the shared catalog the watcher observes.
    let down = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "down",
            "alpha",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux down alpha");
    assert!(down.status.success(), "down should succeed: {down:?}");

    // Edit the (downed) bundle file: the watcher must absorb the change without
    // restarting the runtime.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(true),
    );
    let suppressed = poll_inscription_event(
        &inscriptions_root,
        "relay.bundle.reload_suppressed_held",
        "alpha",
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        suppressed,
        "expected relay.bundle.reload_suppressed_held for the downed bundle; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    // The suppressed edit must not have reloaded (and thereby restarted) the
    // bundle the operator deliberately took down.
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let reloaded = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.reloaded" && entry["details"]["bundle_name"] == "alpha"
        });
    assert!(
        !reloaded,
        "downed bundle must not be reloaded on a file edit: {inscriptions}"
    );
}

// A bundle configured `autostart = false` enters the catalog already held, so a
// configuration edit must not start it: the watcher records
// `relay.bundle.reload_suppressed_held` and does not reload. This is the static
// (declared) counterpart to an explicit `down` — the same hold rule, sourced
// from config rather than a runtime command, with no `down` issued.
#[test]
fn host_relay_watcher_holds_non_autostart_bundle_on_file_edit() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(false),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // Edit the never-started bundle file: the standing `autostart = false` hold
    // must keep the watcher from bringing it up.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(false),
    );
    let suppressed = poll_inscription_event(
        &inscriptions_root,
        "relay.bundle.reload_suppressed_held",
        "alpha",
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        suppressed,
        "expected relay.bundle.reload_suppressed_held for the non-autostart bundle; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let reloaded = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.reloaded" && entry["details"]["bundle_name"] == "alpha"
        });
    assert!(
        !reloaded,
        "non-autostart bundle must not be reloaded on a file edit: {inscriptions}"
    );
}

// With `--no-watch`, a bundle file added at runtime is not reconciled: the relay
// continues to reject connections to the new bundle until a restart.
#[test]
fn host_relay_no_watch_flag_disables_runtime_reconcile() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-watch",
            "--config-directory",
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
        .expect("spawn agentmux host relay --no-watch");
    wait_for_relay_ready(&state_root, "alpha");

    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    // Give a watcher (if one existed) ample time to reconcile, then confirm the
    // bundle is still unknown: no reconciliation happened.
    thread::sleep(WATCHER_RECONCILE_BUDGET);
    let frame = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}

// relay.toml `watch-bundles = false` disables the watcher without the `--no-watch`
// CLI flag: a bundle added at runtime is not reconciled, mirroring the CLI-flag
// behavior but sourced from configuration.
#[test]
fn host_relay_watch_bundles_false_from_relay_toml_disables_reconcile() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    fs::write(config_root.join("relay.toml"), "watch-bundles = false\n").expect("write relay.toml");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay with relay.toml watch-bundles=false");
    wait_for_relay_ready(&state_root, "alpha");

    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    // Give a watcher (if one existed) ample time to reconcile, then confirm the
    // bundle is still unknown: no reconciliation happened.
    thread::sleep(WATCHER_RECONCILE_BUDGET);
    let frame = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = process::wait_with_output_bounded(child, process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}
