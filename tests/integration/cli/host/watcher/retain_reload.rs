use std::{
    fs,
    process::{Command, Stdio},
};

use serde_json::Value;
use tempfile::TempDir;

use super::super::*;
use super::hello_keepalive::*;
use super::inscriptions::*;
use crate::support::permissions::{deny_directory_access, report_permission_fixture_skip};

// A layer that becomes unreadable at runtime holds the last successful
// reconciliation rather than tearing the catalog down. Enumeration is ground
// truth for the unload pass, so a layer that cannot be enumerated reads as every
// bundle in it having been deleted — the relay would evict live sessions and
// unload a running bundle over a permission bit, and the only trace would be an
// ordinary unload. Retention is what makes the difference observable: the bundle
// stays connectable and the suppression is inscribed.
#[test]
fn host_relay_watcher_retains_the_catalog_when_a_layer_becomes_unreadable() {
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

    // Denying access to the watched bundles directory is itself a filesystem
    // event, so it drives the reconcile pass under test rather than merely
    // arranging the state for one.
    let bundles = config_root.join("bundles");
    let Some(restore) = deny_directory_access(&bundles) else {
        report_permission_fixture_skip(
            "host_relay_watcher_retains_the_catalog_when_a_layer_becomes_unreadable",
        );
        shutdown_relay_if_present(&state_root, "alpha");
        let _ = child.wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT);
        return;
    };

    let suppressed = poll_inscription_event_kind(
        &inscriptions_root,
        "relay.bundle.reconcile_suppressed_unreadable_layer",
    );
    // Admission reads the bundle file, which lives inside the directory just
    // denied, so this connection cannot be admitted either way. Its *error code*
    // is what separates the two worlds: a retained catalog still knows the
    // bundle and fails on the layer it cannot read, while a torn-down one would
    // have forgotten it and answered `validation_unknown_bundle` — the same
    // answer it gives for a bundle the operator deleted.
    let during = relay_hello_first_frame(&state_root, "a@alpha", "socket-trust");
    // Restored before shutdown so the relay's own teardown is not fighting a
    // directory it cannot traverse.
    drop(restore);
    // Readable again, and serving without an intervening reload: the runtime
    // that answers here is the one that was running before the layer went dark.
    let after = poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    assert!(
        suppressed,
        "expected relay.bundle.reconcile_suppressed_unreadable_layer; relay inscriptions: {inscriptions}"
    );
    assert_eq!(
        during["response"]["error"]["code"], "validation_unreadable_configuration_layer",
        "a retained bundle must fail on the unreadable layer rather than be forgotten: \
         {during:?}; relay inscriptions: {inscriptions}"
    );
    assert_eq!(
        after["frame"], "hello_ack",
        "the bundle must still be served once the layer is readable again: {after:?}; \
         relay inscriptions: {inscriptions}"
    );
    let disturbed: Vec<String> = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["details"]["bundle_name"] == "alpha")
        .filter_map(|entry| {
            entry["event"]
                .as_str()
                .filter(|event| {
                    matches!(
                        *event,
                        "relay.bundle.unloaded" | "relay.bundle.reloaded" | "relay.bundle.loaded"
                    )
                })
                .map(str::to_string)
        })
        .collect();
    assert!(
        disturbed.is_empty(),
        "an unreadable layer must disturb neither the catalog nor the runtime, saw {disturbed:?}: \
         {inscriptions}"
    );
}

// A bundle TOML file modified at runtime is treated as a full teardown +
// reload: active sessions receive a `runtime_bundle_reloaded` error frame before
// disconnect, and the relay reloads the bundle (a fresh connection succeeds).
#[test]
fn host_relay_watcher_reloads_modified_bundle_file_at_runtime() {
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
    // Modify the alpha bundle file (add a session): the watcher reloads it.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(true),
    );
    let eviction = expect_watcher_signal(
        read_next_frame(&mut reader),
        "reload eviction frame",
        &inscriptions_root,
    );
    // After reload the bundle is still loaded: a fresh Hello succeeds.
    let after = poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

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
        "runtime_bundle_reloaded"
    );
    assert_eq!(
        after["frame"],
        "hello_ack",
        "expected hello_ack after reload: {after:?}; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(after["principal_id"], "a@alpha");
}

// A definition appearing in an earlier layer over a byte-identical one in a
// later layer — and later being removed to reveal it again — changes which file
// supplies the identifier without changing a single content byte. Both
// transitions must reload, since the relay now tracks a different file for edits
// and deletions.
#[test]
fn host_relay_watcher_reloads_when_a_byte_identical_earlier_layer_appears_and_is_removed() {
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
    // A sibling layer, watched from startup so its bundles directory appearing
    // later is observed. The bundle file itself arrives mid-test.
    let override_layer = temporary.path().join("override");
    let override_bundles = override_layer.join("bundles");
    fs::create_dir_all(&override_bundles).expect("create override bundles");
    let base_bundle = config_root.join("bundles/alpha.toml");
    let override_bundle = override_bundles.join("alpha.toml");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &override_layer.to_string_lossy(),
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

    let (_appearance_stream, mut appearance_reader, appearance_hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    fs::copy(&base_bundle, &override_bundle).expect("copy base bundle into the earlier layer");
    let appearance_eviction = expect_watcher_signal(
        read_next_frame(&mut appearance_reader),
        "earlier-layer appearance reload eviction frame",
        &inscriptions_root,
    );
    let after_appearance =
        poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
            frame["frame"] == "hello_ack"
        });

    let (_removal_stream, mut removal_reader, removal_hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    fs::remove_file(&override_bundle).expect("remove the earlier-layer bundle");
    let removal_eviction = expect_watcher_signal(
        read_next_frame(&mut removal_reader),
        "earlier-layer removal reload eviction frame",
        &inscriptions_root,
    );
    let after_removal = poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(appearance_hello["frame"], "hello_ack");
    assert_eq!(
        appearance_eviction["response"]["error"]["code"], "runtime_bundle_reloaded",
        "an earlier layer appearing over identical base content must reload: \
         {appearance_eviction:?}"
    );
    assert_eq!(after_appearance["frame"], "hello_ack");
    assert_eq!(removal_hello["frame"], "hello_ack");
    assert_eq!(
        removal_eviction["response"]["error"]["code"], "runtime_bundle_reloaded",
        "removing an earlier layer to reveal identical base content must reload: \
         {removal_eviction:?}"
    );
    assert_eq!(after_removal["frame"], "hello_ack");
}
