//! The lazy Pty spawn path, which reaches its member through delivery rather
//! than through bring-up.
//!
//! A Pty member that was not started at bring-up is spawned when a delivery
//! first targets it, from a bundle the delivery layer loads itself. That made
//! it a third injection path, distinct from first startup and `up`/reconcile,
//! and the one where an authoritative stamp applied only at bring-up would go
//! missing.
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
fn write_pty_bundle(config_root: &std::path::Path, report: &std::path::Path, declared: &str) {
    // Reuse the shared scaffolding for coders/policies/users, then replace the
    // bundle file with a Pty member.
    write_bundle_configuration_with_options(config_root, "alpha", None, &["a"], Some(false));
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

/// Ignored because the path it exercises cannot complete yet, not because the
/// expectation is wrong. `PtyTransport::startup` spawns the child, then blocks on
/// the worker's init handshake — and that block is a tokio `blocking_recv` on a
/// runtime worker thread, which panics with "Cannot block the current thread from
/// within a runtime". So the child *is* spawned, and is then killed by the
/// startup guard as the panic unwinds.
///
/// That ordering is why this cannot be made to pass by observing the child: the
/// window between spawn and kill is not synchronized with anything the test can
/// wait on, so a test that reads the child's report would pass or fail on
/// timing. It stays ignored rather than becoming flaky.
///
/// Tracked separately as `agentmux:issues/runtime/8` — the defect is independent
/// of state-root propagation and predates it. Remove the ignore once the spawn
/// completes; the assertion needs no change.
fn assert_lazy_pty_spawn_carries_the_relays_state_root(declared: &str) {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    let report = temporary.path().join("child-state-directory");
    fs::create_dir_all(config_root.join("bundles")).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_pty_bundle(&config_root, &report, declared);

    // `--no-autostart` plus `autostart = false` keeps bring-up from starting the
    // member, so the spawn can only come from the delivery below. That is the
    // path under test; starting it at bring-up would prove the wrong thing.
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

    // Hosts the bundle so delivery can route to it. Reconcile creates only Tmux
    // members, so the Pty member is still unstarted afterwards — which is what
    // makes the spawn below come from delivery rather than from bring-up.
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
    assert!(
        !report.exists(),
        "bring-up must not have spawned the Pty member; the lazy path is under test"
    );

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

    let reported = await_report(&report);

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();

    assert_eq!(
        reported,
        state_root.display().to_string(),
        "the lazily spawned Pty child must carry the spawning relay's state root, \
         not its own declaration"
    );
}

#[ignore = "lazy Pty spawn panics in a tokio worker; agentmux:issues/runtime/8"]
#[test]
fn a_lazily_spawned_pty_member_receives_the_relays_state_root() {
    assert_lazy_pty_spawn_carries_the_relays_state_root(MEMBER_DECLARED_STATE_ROOT);
}

// The blank declaration, which reads as absent everywhere else and so is the case
// an upsert-if-absent implementation would leave to the child.
#[ignore = "lazy Pty spawn panics in a tokio worker; agentmux:issues/runtime/8"]
#[test]
fn a_lazily_spawned_pty_member_survives_a_blank_declaration() {
    assert_lazy_pty_spawn_carries_the_relays_state_root(MEMBER_BLANK_STATE_ROOT);
}
