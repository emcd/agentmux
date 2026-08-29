use std::{
    fs,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use tempfile::TempDir;

use super::super::super::helpers::*;
use super::super::*;

/// Negative-assertion budget for `--no-watch` and `watch-bundles = false`
/// tests: how long to wait after writing a runtime bundle before asserting
/// the relay still reports it as unknown. A watcher (if one existed)
/// would have had this long to reconcile. The watcher polls on the
/// bundle-watcher's internal interval (well under 1 second even on
/// slow CI); 1 second is generous margin to ensure a missing watcher
/// is correctly distinguished from a slow watcher.
const WATCHER_RECONCILE_BUDGET: Duration = Duration::from_secs(1);

// With `--no-watch`, a bundle file added at runtime is not reconciled: the relay
// continues to reject connections to the new bundle until a restart.
#[test]
fn host_relay_no_watch_flag_disables_runtime_reconcile() {
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
                "--no-watch",
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
            .expect("spawn agentmux host relay --no-watch"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    // Give a watcher (if one existed) ample time to reconcile, then confirm the
    // bundle is still unknown: no reconciliation happened.
    thread::sleep(WATCHER_RECONCILE_BUDGET);
    let frame = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}

// relay.toml `watch-bundles = false` disables the watcher without the `--no-watch`
// CLI flag: a bundle added at runtime is not reconciled, mirroring the CLI-flag
// behavior but sourced from configuration.
#[test]
fn host_relay_watch_bundles_false_from_relay_toml_disables_reconcile() {
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
    fs::write(config_root.join("relay.toml"), "watch-bundles = false\n").expect("write relay.toml");
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
            .expect("spawn agentmux host relay with relay.toml watch-bundles=false"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    // Give a watcher (if one existed) ample time to reconcile, then confirm the
    // bundle is still unknown: no reconciliation happened.
    thread::sleep(WATCHER_RECONCILE_BUDGET);
    let frame = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}
