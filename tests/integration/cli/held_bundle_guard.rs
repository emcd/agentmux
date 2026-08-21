//! Held-bundle delivery guard — transport-agnostic coverage in the default suite.
//!
//! The guard is not a Pty change. It alters delivery for every transport,
//! and the spec scenario "Held check is transport-agnostic" has no Pty-gated
//! test. A failure here must be caught without `--features pty`.

use std::{
    fs,
    process::{Command, Stdio},
};

use tempfile::TempDir;

use super::super::support::process;
use super::helpers::*;

#[test]
fn a_send_to_a_held_tmux_bundle_is_rejected_as_held() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(config_root.join("bundles")).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    // Tmux bundle, autostart = false so it stays held (HostingIntent::Hold).
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

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
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    // Do NOT run `up alpha` — bundle stays held.
    let send = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--target",
            "a@alpha",
            "--message",
            "wake",
            "--as-session",
            "user@GLOBAL",
            "--bundle",
            "alpha",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux send to held bundle");
    let stderr = String::from_utf8_lossy(&send.stderr);
    let stdout = String::from_utf8_lossy(&send.stdout);
    assert!(
        stderr.contains("runtime_bundle_held") || stdout.contains("runtime_bundle_held"),
        "send to a held bundle must resolve as held/unavailable, not queued; stderr: {stderr} stdout: {stdout}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn a_raww_to_a_held_tmux_bundle_is_rejected_as_held() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(config_root.join("bundles")).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

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
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let raww = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "raww",
            "a@alpha",
            "--text",
            "hello",
            "--as-session",
            "user@GLOBAL",
            "--bundle",
            "alpha",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux raww to held bundle");
    let stderr = String::from_utf8_lossy(&raww.stderr);
    let stdout = String::from_utf8_lossy(&raww.stdout);
    assert!(
        stderr.contains("runtime_bundle_held") || stdout.contains("runtime_bundle_held"),
        "raww to a held bundle must resolve as held/unavailable; stderr: {stderr} stdout: {stdout}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}
