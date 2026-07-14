//! `--require-credentials` CLI flag, `require-session-credentials = true` in
//! `relay.toml`, and the absence of either.

use std::{
    fs,
    process::{Command, Stdio},
};

use tempfile::TempDir;

use super::*;

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
