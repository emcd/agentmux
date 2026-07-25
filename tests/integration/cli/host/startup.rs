//! Startup modes (default autostart, no-autostart process-only), startup-
//! failure records surfaced through `list`, summary folding of per-session
//! failure reasons, startup-failure clearing on successful session startup,
//! and per-bundle failure detail inscription.

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::Value;
use tempfile::TempDir;

use super::*;

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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");

    let listed = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "principals",
            "--namespace",
            "alpha",
            "--json",
            "--configuration-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_ready(&state_root, "alpha");

    let listed = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "principals",
            "--namespace",
            "alpha",
            "--json",
            "--configuration-directory",
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
            "--configuration-directory",
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
