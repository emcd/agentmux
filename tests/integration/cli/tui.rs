use std::{fs, process::Command};

use tempfile::TempDir;

use super::helpers::{write_bundle_configuration, write_tui_configuration};

#[test]
fn tui_rejects_sender_flag_in_mvp() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["tui", "--sender", "relay"])
        .output()
        .expect("run agentmux tui with sender flag");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid argument --sender"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn tui_rejects_missing_default_bundle_when_bundle_is_omitted() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &["alpha", "bravo"],
    );
    write_tui_configuration(
        &config_root,
        None,
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "tui",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux tui");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_bundle"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn tui_rejects_default_session_with_unknown_policy() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &["alpha", "bravo"],
    );
    write_tui_configuration(
        &config_root,
        Some("agentmux"),
        Some("user"),
        &[("user", "missing", Some("Operator"))],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "tui",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux tui");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_policy"),
        "unexpected stderr: {stderr}"
    );
}
