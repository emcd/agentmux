use std::{
    fs,
    process::{Command, Stdio},
};

use serde_json::Value;
use tempfile::TempDir;

use super::super::super::helpers::*;
use super::super::*;
use super::hello_keepalive::*;
use super::inscriptions::*;

// A bundle TOML file added at runtime is picked up by the watcher: the relay
// loads and starts the bundle without a restart, and a new connection to that
// bundle succeeds where it was previously rejected as an unknown bundle.
#[test]
fn host_relay_watcher_loads_new_bundle_file_at_runtime() {
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

    // The bravo bundle does not exist yet: Hello is rejected as unknown.
    let before = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");
    // Add the bravo bundle file at runtime; the watcher should load it.
    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    let added = poll_hello_first_frame(&state_root, "b@bravo", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        before["frame"], "response",
        "expected error frame: {before:?}"
    );
    assert_eq!(
        before["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
    assert_eq!(
        added["frame"],
        "hello_ack",
        "expected hello_ack after add: {added:?}; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(added["principal_id"], "b@bravo");
}

// A bundle TOML file added at runtime with `autostart = false` is loaded held:
// the relay learns it (its members register as offline shells, so Hello is
// accepted) but does not start it, mirroring the boot-time process-only path.
#[test]
fn host_relay_watcher_loads_non_autostart_bundle_held_without_starting() {
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

    // Add a non-autostart bravo bundle at runtime; the watcher should register it
    // held rather than start it.
    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(false),
    );
    let loaded_held =
        poll_inscription_event(&inscriptions_root, "relay.bundle.loaded_held", "bravo");
    // The held bundle is nonetheless known: Hello to one of its members succeeds.
    let known = poll_hello_first_frame(&state_root, "b@bravo", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        loaded_held,
        "expected relay.bundle.loaded_held for the non-autostart runtime add; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(
        known["frame"], "hello_ack",
        "held bundle should still be known: {known:?}"
    );
    // It must not have been started: no `relay.bundle.loaded` for bravo.
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let started = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.loaded" && entry["details"]["bundle_name"] == "bravo"
        });
    assert!(
        !started,
        "non-autostart bundle must not be started on runtime add: {inscriptions}"
    );
}

// A bundle TOML file removed at runtime unloads the bundle: active sessions
// receive a `runtime_bundle_unloaded` error frame before disconnect, and
// subsequent connection attempts to that bundle are rejected as unknown.
#[test]
fn host_relay_watcher_unloads_removed_bundle_file_at_runtime() {
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

    let (_stream, mut reader, hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    fs::remove_file(config_root.join("bundles").join("alpha.toml")).expect("remove bundle file");
    // The active session's connection receives the typed unload frame.
    let eviction = expect_watcher_signal(
        read_next_frame(&mut reader),
        "unload eviction frame",
        &inscriptions_root,
    );
    // The catalog is updated before the eviction frame is written, so a new
    // Hello to the unloaded bundle is now rejected as unknown.
    let after = relay_hello_first_frame(&state_root, "a@alpha", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(hello["frame"], "hello_ack", "expected hello_ack: {hello:?}");
    assert_eq!(
        eviction["frame"], "response",
        "expected eviction frame: {eviction:?}"
    );
    assert_eq!(
        eviction["response"]["error"]["code"],
        "runtime_bundle_unloaded"
    );
    assert_eq!(
        after["frame"], "response",
        "expected unknown-bundle frame: {after:?}"
    );
    assert_eq!(
        after["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}
