//! CLI coverage for `agentmux check configuration` — the read-only
//! configuration pre-flight subcommand.

use std::{fs, process::Command};

use tempfile::TempDir;

use super::helpers::*;

fn config_and_state(temporary: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    (config_root, state_root)
}

#[test]
fn check_configuration_accepts_valid_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "alpha",
            "--config-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        output.status.success(),
        "command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok: alpha"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("all valid"), "unexpected stdout: {stdout}");
}

#[test]
fn check_configuration_validates_all_bundles_when_no_id() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    write_bundle_configuration(&config_root, "bravo", None, &["b"]);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--config-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        output.status.success(),
        "command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok: alpha"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("ok: bravo"), "unexpected stdout: {stdout}");
}

#[test]
fn check_configuration_reports_unknown_field_with_detail() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    // Overwrite the bundle with the headline incident: a misspelled session key
    // (`codex-session-id` instead of `coder`). `deny_unknown_fields` rejects it.
    fs::write(
        config_root.join("bundles").join("alpha.toml"),
        r#"format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "/tmp"
coder = "default"
codex-session-id = "a"
"#,
    )
    .expect("overwrite bundle with bad field");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "alpha",
            "--config-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("codex-session-id"),
        "stderr should name the offending field: {stderr}"
    );
    assert!(
        stderr.contains("alpha.toml"),
        "stderr should name the offending file: {stderr}"
    );
}

#[test]
fn check_configuration_reports_no_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--config-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no bundle configurations found"),
        "unexpected stderr: {stderr}"
    );
}

// A malformed relay.toml is reported even when the config root has no bundles:
// relay-level validation runs before bundle discovery, matching relay startup
// (which rejects the same artifact up front) rather than short-circuiting on the
// no-bundles error.
#[test]
fn check_configuration_reports_invalid_relay_toml_without_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    fs::write(
        config_root.join("relay.toml"),
        "relay-id = \"this-relay\"\n[[peers]]\nid = \"peer-relay@RELAY\"\naddress = \"\"\n",
    )
    .expect("write relay.toml");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--config-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("peers.address"),
        "stderr should name the offending relay.toml field: {stderr}"
    );
    assert!(
        !stderr.contains("no bundle configurations found"),
        "relay.toml validation must run before the no-bundles check: {stderr}"
    );
}

#[test]
fn check_configuration_rejects_unknown_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["check", "everything"])
        .output()
        .expect("run agentmux check everything");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown check subcommand"),
        "unexpected stderr: {stderr}"
    );
}
