//! Pty bring-up through the public CLI paths: `up` elects the member's
//! delivery worker, and what follows pins the externally observable halves
//! of that contract.
//!
//! - An eagerly elected worker serves deliveries with no ACP-flavored queue
//!   cap: seventy sends to a prompt-blocked target all succeed, which the
//!   `ACP_PENDING_MAX` cap would refuse from the sixty-fifth on.
//! - A construction failure surfaces with the transport's own code, for one
//!   bring-up and for two racing it, and a fixed config heals on the next
//!   bring-up.
//!
//! Mechanism internals that no public path can name — attempt attribution
//! under shared and replaced owners — are pinned beside the table in
//! `async_worker::registry` unit tests.

use std::{
    fs,
    process::{Command, Output, Stdio},
    thread,
};

use tempfile::TempDir;

use super::super::support::process;
use super::helpers::*;

const PROMPT_THAT_NEVER_MATCHES: &str = "PROMPT-THAT-NEVER-APPEARS-8f2e1a";

/// Writes a bundle whose single member is a Pty target, with caller-chosen
/// command and prompt gate.
fn write_pty_member_bundle(
    config_root: &std::path::Path,
    initial_command: &str,
    prompt_regex: Option<&str>,
) {
    write_bundle_configuration_with_options(
        config_root,
        "alpha",
        None,
        &["a"],
        Some(false),
    );
    let prompt_line = prompt_regex
        .map(|regex| format!("prompt-regex = \"{regex}\"\n"))
        .unwrap_or_default();
    // Literal string for the command: a tokenization-probing command can
    // carry double quotes, which would terminate a basic string early and
    // fail config load instead of exercising bring-up.
    fs::write(
        config_root.join("coders.toml"),
        format!(
            r#"
format-version = 1

[[coders]]
id = "default"

[coders.pty]
initial-command = '{initial_command}'
resume-command = "sh -lc 'exec sleep 45'"
{prompt_line}"#,
        ),
    )
    .expect("write pty coders config");
    fs::write(
        config_root.join("bundles").join("alpha.toml"),
        r#"format-version = 1
autostart = false

[[sessions]]
id = "a"
name = "a"
directory = "/tmp"
coder = "default"
"#,
    )
    .expect("write pty bundle config");
}

fn hosted_paths(
    temporary: &TempDir,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(config_root.join("bundles"))
        .expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    (config_root, state_root, inscriptions_root)
}

fn spawn_relay(
    config_root: &std::path::Path,
    state_root: &std::path::Path,
    inscriptions_root: &std::path::Path,
) -> process::RelayChildGuard {
    let child = process::RelayChildGuard::new(
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
    wait_for_relay_ready(state_root, "alpha");
    child
}

fn run_up(
    config_root: &std::path::Path,
    state_root: &std::path::Path,
    inscriptions_root: &std::path::Path,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run agentmux up")
}

fn run_send(
    config_root: &std::path::Path,
    state_root: &std::path::Path,
    inscriptions_root: &std::path::Path,
    message: &str,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--target",
            "a@alpha",
            "--message",
            message,
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
        .expect("run agentmux send")
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// An eagerly elected Pty worker serves past the ACP pending cap.
///
/// Seventy sends to a prompt-blocked target all succeed. Slots release
/// only at the terminal ack, and the blocked executor never declares, so
/// every send holds its slot: with the ACP cap in force the sixty-fifth
/// would fail `runtime_acp_queue_full`. The count is exact, not
/// timing-dependent — nothing resolves while the prompt never matches, so
/// no send can free another's slot whatever the scheduling.
#[test]
fn eager_pty_worker_serves_past_the_acp_pending_cap() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root, inscriptions_root) =
        hosted_paths(&temporary);
    write_pty_member_bundle(
        &config_root,
        "sleep 45",
        Some(PROMPT_THAT_NEVER_MATCHES),
    );
    let host_child =
        spawn_relay(&config_root, &state_root, &inscriptions_root);

    let up = run_up(&config_root, &state_root, &inscriptions_root);
    assert!(
        up.status.success(),
        "up brings the prompt-blocked member ready (transport readiness, not prompt); stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    for index in 0..70 {
        let send = run_send(
            &config_root,
            &state_root,
            &inscriptions_root,
            &format!("saturation-probe-{index}"),
        );
        let combined = combined_output(&send);
        assert!(
            send.status.success()
                && !combined.contains("runtime_acp_queue_full"),
            "send {index} of 70 must not hit an ACP queue cap on a Pty target: {combined}",
        );
    }

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

/// Bring-up reports an unconstructible child with the transport's own code.
#[test]
fn up_reports_the_transports_own_code_for_an_unconstructible_child() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root, inscriptions_root) =
        hosted_paths(&temporary);
    write_pty_member_bundle(&config_root, "sh -c \"echo hi", None);
    let host_child =
        spawn_relay(&config_root, &state_root, &inscriptions_root);

    let up = run_up(&config_root, &state_root, &inscriptions_root);
    let combined = combined_output(&up);
    assert!(
        !up.status.success(),
        "up must fail when the Pty child cannot be constructed"
    );
    assert!(
        combined.contains("pty_command_parse_failed"),
        "the failure carries the transport's construction code, not a generic timeout: {combined}",
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

/// Two racing bring-ups agree on one attempt's result.
///
/// The command fails deterministically, so both ups report the transport's
/// code however the elections interleave — shared attempt or separate
/// attempts, adopted or re-elected.
#[test]
fn concurrent_ups_agree_on_one_attempts_result() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root, inscriptions_root) =
        hosted_paths(&temporary);
    write_pty_member_bundle(&config_root, "sh -c \"echo hi", None);
    let host_child =
        spawn_relay(&config_root, &state_root, &inscriptions_root);

    thread::scope(|scope| {
        let first = scope
            .spawn(|| run_up(&config_root, &state_root, &inscriptions_root));
        let second = scope
            .spawn(|| run_up(&config_root, &state_root, &inscriptions_root));
        for (index, up) in
            [first.join(), second.join()].into_iter().enumerate()
        {
            let up = up.expect("bring-up thread joins");
            let combined = combined_output(&up);
            assert!(
                !up.status.success(),
                "racing up {index} must fail on the unconstructible child"
            );
            assert!(
                combined.contains("pty_command_parse_failed"),
                "racing up {index} reports the attempt's own cause: {combined}",
            );
        }
    });

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

/// A fixed config heals on the next bring-up: the failed attempt's record
/// never describes its replacement.
#[test]
fn a_fixed_config_heals_on_the_next_bring_up() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root, inscriptions_root) =
        hosted_paths(&temporary);
    write_pty_member_bundle(&config_root, "sh -c \"echo hi", None);
    let host_child =
        spawn_relay(&config_root, &state_root, &inscriptions_root);

    let failed = run_up(&config_root, &state_root, &inscriptions_root);
    assert!(
        !failed.status.success()
            && combined_output(&failed).contains("pty_command_parse_failed"),
        "the bad config fails bring-up with its own code first"
    );

    write_pty_member_bundle(&config_root, "sleep 45", None);
    let healed = run_up(&config_root, &state_root, &inscriptions_root);
    assert!(
        healed.status.success(),
        "the fixed config brings the member ready; stderr:\n{}",
        String::from_utf8_lossy(&healed.stderr)
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}
