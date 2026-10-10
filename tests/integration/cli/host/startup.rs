//! Startup modes (default autostart, no-autostart process-only) and the
//! autostart summary shape (partial startup reported as degraded, failed
//! session reasons folded into the per-bundle summary).
//!
//! Startup-failure records surfaced through `list` live in
//! [`super::startup_failures`]; the bundle-configuration writers shared by
//! both clusters live in [`super::startup_config`].

use std::{
    fs,
    process::{Command, Stdio},
};

use tempfile::TempDir;

use super::startup_config::{
    write_all_acp_failure_bundle, write_bundle_configuration_with_tmux_and_acp_failure,
};
use super::*;

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
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
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
fn host_relay_autostart_summary_reports_a_partial_startup_as_degraded() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    // One tmux member reaches ready state; one ACP member cannot be spawned.
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
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");

    // A partial startup is still a hosted relay: the process must not fail on
    // account of it.
    assert!(output.status.success(), "command should succeed");
    let summary_json = parse_summary_json_line(&output.stdout);
    let alpha = summary_json["bundles"]
        .as_array()
        .expect("startup summary bundles")
        .iter()
        .find(|bundle| bundle["bundle_name"] == "alpha")
        .expect("alpha startup summary");

    assert_eq!(
        alpha["outcome"], "degraded",
        "a partially started bundle must not report an unqualified hosted: {summary_json}"
    );
    assert_eq!(summary_json["degraded_bundle_count"], 1);
    assert_eq!(summary_json["hosted_bundle_count"], 0);
    assert_eq!(summary_json["failed_bundle_count"], 0);
    assert_eq!(summary_json["hosted_any"], true);
    assert!(
        alpha["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("bravo")),
        "degraded reason should name the failed session: {summary_json}"
    );
    let failed_sessions = alpha["details"]["failed_sessions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected failed_sessions detail: {summary_json}"));
    assert!(
        failed_sessions
            .iter()
            .any(|failure| failure["session_id"] == "bravo"),
        "degraded detail should carry the per-session record: {summary_json}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("session=bravo"),
        "startup output should name the failed session: {stdout}"
    );
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
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
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

    let child = process::RelayChildGuard::new(
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
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
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
