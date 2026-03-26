use std::{
    fs,
    process::{Command, Stdio},
};

use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn up_requires_selector_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["up"])
        .output()
        .expect("run agentmux up");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid argument <bundle-id>|--group: missing selector"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn down_rejects_conflicting_bundle_and_group_selectors() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["down", "alpha", "--group", "dev"])
        .output()
        .expect("run agentmux down with conflicting selectors");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_selectors"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn up_and_down_report_idempotent_transitions() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
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

    let first_up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
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
        .expect("run first up");
    assert!(first_up.status.success(), "first up should succeed");
    let first_up_json = parse_summary_json_line(&first_up.stdout);
    assert_eq!(first_up_json["action"], "up");
    assert_eq!(first_up_json["changed_any"], true);
    assert_eq!(first_up_json["bundles"][0]["outcome"], "hosted");

    let second_up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
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
        .expect("run second up");
    assert!(second_up.status.success(), "second up should succeed");
    let second_up_json = parse_summary_json_line(&second_up.stdout);
    assert_eq!(second_up_json["changed_any"], false);
    assert_eq!(second_up_json["bundles"][0]["outcome"], "skipped");
    assert_eq!(
        second_up_json["bundles"][0]["reason_code"],
        "already_hosted"
    );

    let first_down = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run first down");
    assert!(first_down.status.success(), "first down should succeed");
    let first_down_json = parse_summary_json_line(&first_down.stdout);
    assert_eq!(first_down_json["action"], "down");
    assert_eq!(first_down_json["bundles"][0]["outcome"], "unhosted");

    let second_down = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run second down");
    assert!(second_down.status.success(), "second down should succeed");
    let second_down_json = parse_summary_json_line(&second_down.stdout);
    assert_eq!(second_down_json["bundles"][0]["outcome"], "skipped");
    assert_eq!(
        second_down_json["bundles"][0]["reason_code"],
        "already_unhosted"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let host_output = host_child.wait_with_output().expect("wait for relay host");
    assert!(host_output.status.success(), "host should succeed");
}

#[test]
fn down_reports_relay_unavailable_when_relay_is_not_running() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .output()
        .expect("run down without relay");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("relay_unavailable"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_summary_json_omits_group_name() {
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
    wait_for_relay_socket(&state_root, "alpha");
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output()
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary_line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{') && line.contains("\"host_mode\""))
        .expect("find startup summary json line");
    let payload: Value = serde_json::from_str(summary_line).expect("parse summary payload");
    let payload_object = payload.as_object().expect("summary payload object");
    assert_eq!(
        payload_object
            .get("host_mode")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "autostart"
    );
    assert!(
        !payload_object.contains_key("group_name"),
        "group_name should be omitted in single-bundle mode payload: {payload_object:?}"
    );
}
