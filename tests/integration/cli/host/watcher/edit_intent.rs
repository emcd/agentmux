use std::{
    fs,
    process::{Command, Stdio},
};

use serde_json::Value;
use tempfile::TempDir;

use super::super::*;
use super::inscriptions::*;

// A bundle the operator explicitly took down via `down` is not silently brought
// back up when its configuration file is edited: the watcher absorbs the new
// fingerprint and records `relay.bundle.reload_suppressed_downed` instead of
// reloading (and restarting) the runtime.
#[test]
fn host_relay_watcher_preserves_down_intent_across_file_edit() {
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
    assert_bundle_watch_started(&inscriptions_root);

    // Operator takes the bundle down. This flows over the live relay socket and
    // records the down intent on the shared catalog the watcher observes.
    let down = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "down",
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
        .expect("run agentmux down alpha");
    assert!(down.status.success(), "down should succeed: {down:?}");

    // Edit the (downed) bundle file: the watcher must absorb the change without
    // restarting the runtime.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(true),
    );
    let suppressed = poll_inscription_event(
        &inscriptions_root,
        "relay.bundle.reload_suppressed_held",
        "alpha",
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        suppressed,
        "expected relay.bundle.reload_suppressed_held for the downed bundle; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    // The suppressed edit must not have reloaded (and thereby restarted) the
    // bundle the operator deliberately took down.
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let reloaded = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.reloaded" && entry["details"]["bundle_name"] == "alpha"
        });
    assert!(
        !reloaded,
        "downed bundle must not be reloaded on a file edit: {inscriptions}"
    );
}

// A bundle configured `autostart = false` enters the catalog already held, so a
// configuration edit must not start it: the watcher records
// `relay.bundle.reload_suppressed_held` and does not reload. This is the static
// (declared) counterpart to an explicit `down` — the same hold rule, sourced
// from config rather than a runtime command, with no `down` issued.
#[test]
fn host_relay_watcher_holds_non_autostart_bundle_on_file_edit() {
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
    assert_bundle_watch_started(&inscriptions_root);

    // Edit the never-started bundle file: the standing `autostart = false` hold
    // must keep the watcher from bringing it up.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(false),
    );
    let suppressed = poll_inscription_event(
        &inscriptions_root,
        "relay.bundle.reload_suppressed_held",
        "alpha",
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        suppressed,
        "expected relay.bundle.reload_suppressed_held for the non-autostart bundle; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let reloaded = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.reloaded" && entry["details"]["bundle_name"] == "alpha"
        });
    assert!(
        !reloaded,
        "non-autostart bundle must not be reloaded on a file edit: {inscriptions}"
    );
}
