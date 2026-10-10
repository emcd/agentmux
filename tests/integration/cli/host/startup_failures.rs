//! Startup-failure records surfaced through `list`, startup-failure clearing
//! on successful session startup, the independence of the health verdict from
//! the failure history, and per-bundle failure detail inscription.
//!
//! The bundle-configuration writers shared with [`super::startup`] live in
//! [`super::startup_config`].

use std::{
    fs,
    process::{Command, Stdio},
};

use agentmux::relay::{ListedSessionTransport, StartupFailureRecord, append_startup_failure};
use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::Value;
use tempfile::TempDir;

use super::startup_config::{
    write_bundle_configuration_with_invalid_policy_scope,
    write_bundle_configuration_with_kill_after_create_member,
    write_bundle_configuration_with_tmux_and_acp_failure,
};
use super::*;

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

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
            .expect("spawn agentmux host relay"),
    );
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
        bravo_failure["code"], "runtime_acp_spawn_permanent",
        "bravo startup failure should carry the permanent spawn-failure code, not the \
         generic unavailable placeholder: {listed_json}"
    );
    assert!(
        bravo_failure["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("spawn ACP stdio command failed")),
        "bravo startup failure should surface the true spawn cause: {listed_json}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
}

/// A session whose child dies between `new-session -P` and the ownership
/// mark must surface a startup failure naming it — never a silent success
/// with no session behind it.
#[test]
fn host_relay_records_startup_failure_when_session_dies_before_marking() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_kill_after_create_member(&config_root, "alpha");
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);
    kill_fake_tmux_session_after_create(&fake_tmux, "victim");

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
            .expect("spawn agentmux host relay"),
    );
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
    let victim_failure = failures
        .iter()
        .find(|entry| entry["session_id"] == "victim")
        .unwrap_or_else(|| {
            panic!("expected startup failure for the instant-death session: {listed_json}")
        });
    // The captured `$N` mark must fail against the dead session rather than
    // land on a sibling: the record carries the failed mark, not silence.
    let failure_text = serde_json::to_string(victim_failure).expect("encode failure record");
    assert!(
        failure_text.contains("no such session"),
        "instant-death failure should name the failed mark: {listed_json}"
    );
    assert!(
        !failures.iter().any(|entry| entry["session_id"] == "steady"),
        "the healthy member must not carry a failure: {listed_json}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
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

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
            .expect("spawn agentmux host relay"),
    );
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
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
}

// Replays the 2026-06-11 outage shape: every autostart bundle fails (here from
// a policy validation rejection) and the host exits. The journal (stderr) and
// the inscription log must each carry a per-bundle reason with the structured
// error details, not just the aggregate bundle count.
#[test]
fn host_relay_list_reports_healthy_while_a_startup_failure_record_stands() {
    // Health and the failure history answer different questions, and this is the
    // one direction from which their independence can be demonstrated rather
    // than asserted: every configured session is ready, a record is still on
    // file, and the verdict is `healthy`. An implementation that consulted the
    // history to decide health would report `degraded` here.
    //
    // The record is written *after* bring-up on purpose. Seeding it beforehand
    // would have the session's own successful startup clear it, which is a
    // different behaviour (covered above) and would leave this test asserting
    // `healthy` against an empty history — true for the wrong reason, and true
    // even of an implementation that reads the log.
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

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let runtime_directory = BundleRuntimePaths::resolve(&state_root, "alpha")
        .expect("resolve bundle runtime paths")
        .runtime_directory;
    append_startup_failure(
        &runtime_directory,
        StartupFailureRecord {
            session_id: "primary".to_string(),
            transport: ListedSessionTransport::Tmux,
            code: "runtime_startup_failed".to_string(),
            reason: "a failure this session has since recovered from".to_string(),
            timestamp: "2026-05-01T00:00:00Z".to_string(),
            sequence: 0,
            details: None,
        },
    )
    .expect("append startup failure record");

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
        .expect("run list principals");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(listed.status.success(), "list principals should succeed");
    let listed_json: Value = serde_json::from_slice(&listed.stdout).expect("decode list payload");
    // The record has to still be there, or `healthy` proves nothing.
    assert!(
        listed_json["bundle"]["startup_failure_count"]
            .as_u64()
            .expect("startup failure count")
            >= 1,
        "the seeded record must survive to the payload: {listed_json}"
    );
    assert_eq!(listed_json["bundle"]["state"], "up");
    assert_eq!(
        listed_json["bundle"]["startup_health"], "healthy",
        "a ready session's recorded failure must not lower the health verdict: {listed_json}"
    );
}

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
