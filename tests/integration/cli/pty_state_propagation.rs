//! The eager Pty bring-up path, which elects its member's worker at `up`
//! rather than at first delivery.
//!
//! A Pty member is started when its bundle is brought up, from the same
//! registry election a first delivery would run — which is what keeps one
//! worker (and therefore one child) per target. An authoritative stamp
//! applied at bring-up is therefore present for the delivery that follows,
//! closing the third-injection-path gap the lazy spawn used to be.
//!
//! The child reports its own environment into a file, so what is asserted is
//! what the spawned process actually received rather than what configuration
//! said it should.

use std::{
    fs,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::super::support::process;
use super::helpers::*;

/// A state root the member declares for itself, which the relay must overwrite.
const MEMBER_DECLARED_STATE_ROOT: &str = "/nowhere/member-declared";

/// A blank declaration, which must be overwritten rather than suppressing the
/// stamp.
const MEMBER_BLANK_STATE_ROOT: &str = "";

/// Writes a bundle whose single member is a Pty target declaring its own
/// `AGENTMUX_STATE_DIRECTORY`, with an initial command that records the value
/// it was actually spawned with.
fn write_pty_bundle(
    config_root: &std::path::Path,
    report: &std::path::Path,
    declared: &str,
) {
    // Reuse the shared scaffolding for coders/policies/users, then replace the
    // bundle file with a Pty member.
    write_bundle_configuration_with_options(
        config_root,
        "alpha",
        None,
        &["a"],
        Some(false),
    );
    fs::write(
        config_root.join("coders.toml"),
        format!(
            r#"
format-version = 1

[[coders]]
id = "default"

[coders.pty]
initial-command = "sh -lc 'printenv AGENTMUX_STATE_DIRECTORY > {report} || true; exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
            report = report.display()
        ),
    )
    .expect("write pty coders config");
    fs::write(
        config_root.join("bundles").join("alpha.toml"),
        format!(
            r#"format-version = 1
autostart = false

[[sessions]]
id = "a"
name = "a"
directory = "/tmp"
coder = "default"

[[sessions.environment]]
name = "AGENTMUX_STATE_DIRECTORY"
value = "{declared}"
"#
        ),
    )
    .expect("write pty bundle config");
}

/// Waits for the spawned child to report its environment.
fn await_report(report: &std::path::Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(contents) = fs::read_to_string(report)
            && !contents.trim().is_empty()
        {
            return contents.trim().to_string();
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "the Pty child never reported its environment at {}",
        report.display()
    );
}

/// The eagerly started Pty child carries the relay's state root.
///
/// Bring-up (`up`) elects the member's worker and spawns its child; the
/// subsequent delivery is adopted by that same worker rather than spawning
/// a second child. Both the spawn-time environment and the delivery path
/// are exercised: the report proves what the child was spawned with, and
/// the accepted send proves the adopted worker serves.
fn assert_eager_pty_spawn_carries_the_relays_state_root(declared: &str) {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    let report = temporary.path().join("child-state-directory");
    fs::create_dir_all(config_root.join("bundles"))
        .expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_pty_bundle(&config_root, &report, declared);

    // Bring-up starts the member: the worker is elected at `up` and the
    // child spawned in its first generation build. That is the path under
    // test; the spawn happening anywhere else would prove the wrong thing.
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
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "up should host the bundle; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );
    // Eager bring-up spawns the child: the report appears at `up`, not at
    // first delivery. Waiting here (rather than after the send) is what
    // pins the spawn to bring-up.
    let reported = await_report(&report);
    assert_eq!(
        reported,
        state_root.display().to_string(),
        "the eagerly spawned Pty child must carry the spawning relay's state root, \
         not its own declaration"
    );

    // Delivery through the adopted worker: accepted and served by the same
    // worker bring-up elected, not a second spawn.
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
        .expect("run agentmux send");
    assert!(
        send.status.success(),
        "send should be accepted; stderr:\n{}",
        String::from_utf8_lossy(&send.stderr)
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn an_eagerly_started_pty_member_receives_the_relays_state_root() {
    assert_eager_pty_spawn_carries_the_relays_state_root(
        MEMBER_DECLARED_STATE_ROOT,
    );
}

// The blank declaration, which reads as absent everywhere else and so is the case
// an upsert-if-absent implementation would leave to the child.
#[test]
fn an_eagerly_started_pty_member_survives_a_blank_declaration() {
    assert_eager_pty_spawn_carries_the_relays_state_root(
        MEMBER_BLANK_STATE_ROOT,
    );
}

#[test]
fn a_send_to_a_held_pty_bundle_does_not_spawn_and_resolves_held() {
    // Held-bundle guard (issues/runtime/9):
    // a delivery to a member of a bundle whose HostingIntent is Hold must not
    // spawn a worker, even though the lazy Pty path would otherwise do so.
    // Today a held Pty bundle spawns then panics (group 3 unmasked by group 1);
    // after the guard it must reject as held/unavailable.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    let report = temporary.path().join("child-state-directory");
    fs::create_dir_all(config_root.join("bundles"))
        .expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_pty_bundle(&config_root, &report, MEMBER_DECLARED_STATE_ROOT);

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

    // Do NOT run `up alpha` — the bundle stays held (autostart = false, catalog
    // entry is Hold). A delivery must not spawn for it.
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
    // Held should be a structured relay error, not a successful queued send.
    // The exact surface is a relay error (runtime_bundle_held) rather than a
    // SendResult; assert it is not a stranded queued delivery.
    let stderr = String::from_utf8_lossy(&send.stderr);
    let stdout = String::from_utf8_lossy(&send.stdout);
    assert!(
        stderr.contains("runtime_bundle_held")
            || stdout.contains("runtime_bundle_held"),
        "send to a held bundle must resolve as held/unavailable, not queued; stderr: {stderr} stdout: {stdout}"
    );
    // Must not have spawned the Pty child — the lazy path is blocked by the
    // is_held check at group construction, before any build_worker_transport.
    assert!(
        !report.exists(),
        "a held Pty bundle must not spawn a child via delivery; report at {} should not exist",
        report.display()
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}
