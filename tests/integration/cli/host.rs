use std::{
    fs,
    process::{Command, Stdio},
};

use tempfile::TempDir;

use super::helpers::*;

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
fn host_relay_rejects_all_flag_in_group_mvp() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--all"])
        .output()
        .expect("run agentmux host relay --all");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_arguments"),
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
    wait_for_relay_socket(&state_root, "alpha");
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output()
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
    wait_for_relay_socket(&state_root, "alpha");
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output()
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

#[test]
fn host_relay_no_autostart_does_not_report_failed_bundle_starts_when_listener_setup_fails() {
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
    fs::create_dir_all(state_root.join("bundles").join("alpha").join("relay.sock"))
        .expect("create blocking relay socket directory");

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .output()
        .expect("run agentmux host relay --no-autostart");

    assert!(output.status.success(), "command should succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("runtime_startup_failed"),
        "unexpected stderr: {stderr}"
    );

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
