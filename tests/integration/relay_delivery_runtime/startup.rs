//! Relay bring-up: retrying transient tmux create failures, and the
//! environment the owned tmux session is created with.

use std::{
    fs,
    time::{Duration, Instant},
};

use tempfile::TempDir;
use tokio::time::{sleep, timeout};

use crate::support::relay_delivery::{
    drain_child_stdout, spawn_relay_with_fake_tmux, wait_for_relay_ready,
    write_bundle_configuration, write_bundle_configuration_with_environment,
    write_fake_tmux_script,
};

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_startup_retries_transient_tmux_create_failures() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");

    let started = Instant::now();
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_ready(&relay_socket).await;
    let elapsed = started.elapsed();

    let stdout = drain_child_stdout(&mut child).await;
    shutdown_relay_gracefully(&mut child).await;

    assert!(
        stdout.contains("\"host_mode\":\"autostart\""),
        "relay should report successful startup, stdout={stdout:?}"
    );
    let attempts = fs::read_to_string(&attempts_file)
        .expect("read attempts")
        .trim()
        .parse::<usize>()
        .expect("parse attempts");
    assert_eq!(attempts, 3, "relay should retry transient create failures");
    assert!(
        elapsed >= Duration::from_millis(50),
        "retry delays should be observable, elapsed={elapsed:?}"
    );
}

/// The merged bundle environment must reach the tmux session-creation call as
/// `new-session -e KEY=VALUE` flags (a plain `Command::env` on the tmux client
/// would not propagate into the pane's child). Boots an autostart bundle whose
/// bundle file declares a top-level `environment`, then asserts the fake tmux's
/// recorded argv for the owned session carries the `-e` flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_creates_tmux_session_with_environment_flags() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_with_environment(
        temporary.path(),
        bundle_name,
        "alpha",
        &[("TMUX_ENV_PROBE", "on")],
    );
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_ready(&relay_socket).await;

    // The autostart reconciler creates the owned session shortly after
    // readiness; poll the recorded argv log until the new-session call lands.
    let deadline = Instant::now() + Duration::from_secs(5);
    let new_session_line = loop {
        let log = fs::read_to_string(&log_file).unwrap_or_default();
        if let Some(line) = log
            .lines()
            .find(|line| line.contains("new-session") && line.contains("-s alpha"))
        {
            break line.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for tmux new-session, log={log:?}"
        );
        sleep(Duration::from_millis(50)).await;
    };
    assert!(
        new_session_line.contains("-e TMUX_ENV_PROBE=on"),
        "new-session must carry the merged environment as -e flags, line={new_session_line:?}"
    );

    let pid = child.id().expect("relay pid");
    let pid = i32::try_from(pid).expect("relay pid fits i32");
    let kill_result = unsafe { libc::kill(pid, libc::SIGINT) };
    assert_eq!(kill_result, 0, "failed to send SIGINT");
    let _ = timeout(Duration::from_secs(3), child.wait()).await;
}
