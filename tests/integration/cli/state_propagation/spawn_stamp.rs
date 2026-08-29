use std::{
    fs,
    process::{Command, Stdio},
};

use tempfile::TempDir;

use super::super::super::support::process;
use super::super::helpers::*;
use super::helpers::*;

/// Brings a relay up with `declared` as the member's own
/// `AGENTMUX_STATE_DIRECTORY` and returns the recorded `new-session`
/// invocation together with the relay's state root.
fn spawn_with_declared_state_root(
    temporary: &TempDir,
    declared: &str,
) -> (String, std::path::PathBuf) {
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
    declare_member_state_directory(&config_root, "alpha", declared);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

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
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
            "alpha",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "up should succeed; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    let new_session = recorded_new_session(&log).to_string();

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
    (new_session, state_root)
}

#[test]
fn a_blank_member_declaration_does_not_suppress_the_stamp() {
    let temporary = TempDir::new().expect("temporary");
    let (new_session, state_root) =
        spawn_with_declared_state_root(&temporary, MEMBER_BLANK_STATE_ROOT);

    assert!(
        new_session.contains(&format!(
            "-e AGENTMUX_STATE_DIRECTORY={}",
            state_root.display()
        )),
        "a blank declaration must be overwritten, not treated as absent-and-left; got:\n\
         {new_session}"
    );
    assert!(
        !new_session.contains("-e AGENTMUX_STATE_DIRECTORY "),
        "no blank value may survive into the spawn; got:\n{new_session}"
    );
}

#[test]
fn a_spawned_member_receives_the_relays_state_root_over_its_own_declaration() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
    declare_member_state_directory(&config_root, "alpha", MEMBER_DECLARED_STATE_ROOT);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

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
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
            "alpha",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux up");
    assert!(up.status.success(), "up should succeed");

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    let new_session = recorded_new_session(&log);

    assert!(
        new_session.contains(&format!(
            "-e AGENTMUX_STATE_DIRECTORY={}",
            state_root.display()
        )),
        "the spawn must carry the relay's state root; got:\n{new_session}"
    );
    assert!(
        !new_session.contains(MEMBER_DECLARED_STATE_ROOT),
        "a member-declared state root must be overwritten, not preserved; got:\n{new_session}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}
