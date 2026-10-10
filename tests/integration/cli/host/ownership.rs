//! Ownership-inscription exactness: the relay marks the tmux session it just
//! created by captured session id, never by session name.
//!
//! A session-name target does prefix matching on real tmux 3.4, so marking
//! an absent shorter name (e.g. `cistella`) can inscribe a surviving longer
//! sibling (e.g. `cistella-o`). The creation path instead prints the new
//! session's id once (`new-session -P -F '#{session_id}'`) and inscribes
//! against that exact handle; when the session disappears first, inscription
//! errors and nothing is recorded.

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::Value;
use tempfile::TempDir;

use super::*;

fn write_cistella_bundle(config_root: &Path) {
    write_bundle_configuration_with_options(config_root, "alpha", None, &["cistella"], Some(true));
}

/// Every malformed creation-id shape refuses inscription through the CLI:
/// empty output, a bare `$`, a non-numeric id, a multiline answer, and
/// trailing garbage each record a startup failure naming the session while
/// the healthy member stays clean and nothing is inscribed.
#[test]
fn malformed_session_id_shapes_all_fail_closed_through_cli() {
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
        None,
        &[
            "steady",
            "shape_empty",
            "shape_dollar",
            "shape_dollarx",
            "shape_multi",
            "shape_trailing",
        ],
        Some(true),
    );

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);
    fail_fake_tmux_session_id_report_raw(&fake_tmux, "shape_empty", "");
    fail_fake_tmux_session_id_report_raw(&fake_tmux, "shape_dollar", "$");
    fail_fake_tmux_session_id_report_raw(&fake_tmux, "shape_dollarx", "$x");
    fail_fake_tmux_session_id_report_raw(&fake_tmux, "shape_multi", "$7\\n$8");
    fail_fake_tmux_session_id_report_raw(&fake_tmux, "shape_trailing", "$7 extra");

    let child = spawn_host_relay(&config_root, &state_root, &inscriptions_root, &fake_tmux);
    wait_for_relay_ready(&state_root, "alpha");

    let listed_json = list_bundle(&state_root, &inscriptions_root, &config_root);
    let failures = listed_json["bundle"]["recent_startup_failures"]
        .as_array()
        .expect("startup failures array");
    for member in [
        "shape_empty",
        "shape_dollar",
        "shape_dollarx",
        "shape_multi",
        "shape_trailing",
    ] {
        let failure = failures
            .iter()
            .find(|entry| entry["session_id"] == member)
            .unwrap_or_else(|| panic!("expected startup failure for {member}: {listed_json}"));
        let failure_text = serde_json::to_string(failure).expect("encode failure record");
        assert!(
            failure_text.contains("did not report a session id"),
            "{member} failure should name the unparseable id: {listed_json}"
        );
    }
    assert!(
        !failures.iter().any(|entry| entry["session_id"] == "steady"),
        "the healthy member must not carry a failure: {listed_json}"
    );
    assert_eq!(
        read_fake_tmux_owned_sessions(&fake_tmux),
        vec!["steady".to_string()],
        "only the healthy member may be inscribed"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
}

fn spawn_host_relay(
    config_root: &Path,
    state_root: &Path,
    inscriptions_root: &Path,
    fake_tmux: &Path,
) -> process::RelayChildGuard {
    process::RelayChildGuard::new(
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
            .env("AGENTMUX_TMUX_COMMAND", fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    )
}

fn list_bundle(state_root: &Path, inscriptions_root: &Path, config_root: &Path) -> Value {
    let listed = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "principals",
            "--namespace",
            "alpha",
            "--json",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run list sessions");
    assert!(listed.status.success(), "list sessions should succeed");
    serde_json::from_slice(&listed.stdout).expect("decode list payload")
}

fn tmux_log(fake_tmux: &Path) -> String {
    fs::read_to_string(fake_tmux_log_path(fake_tmux)).expect("read fake tmux log")
}

fn set_option_targets(log: &str) -> Vec<String> {
    log.lines()
        .filter(|line| line.contains("set-option"))
        .filter_map(|line| {
            let at = line.find("-t ")?;
            line[at + 3..].split_whitespace().next().map(str::to_string)
        })
        .collect()
}

#[test]
fn ownership_inscription_does_not_bleed_to_prefixed_sibling() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_cistella_bundle(&config_root);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);
    // Surviving siblings: `cistella-o` shares the requested `cistella` as a
    // name prefix; `foreign` is an unrelated control.
    fs::write(
        fake_tmux.with_extension("sessions"),
        "cistella-o\nforeign\n",
    )
    .expect("seed fake tmux sessions");
    // The just-created `cistella` dies between the creation and inscription
    // calls — the prefix-match trap the captured id must not fall into.
    kill_fake_tmux_session_after_create(&fake_tmux, "cistella");

    let child = spawn_host_relay(&config_root, &state_root, &inscriptions_root, &fake_tmux);
    wait_for_relay_ready(&state_root, "alpha");

    let listed_json = list_bundle(&state_root, &inscriptions_root, &config_root);
    assert!(
        listed_json["bundle"]["startup_failure_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "expected a startup failure record for the lost session: {listed_json}"
    );

    // With a name-targeting implementation the harness's prefix fallback
    // would record `cistella-o` here; the id path must record nothing.
    let owned = read_fake_tmux_owned_sessions(&fake_tmux);
    assert!(
        !owned
            .iter()
            .any(|name| name == "cistella-o" || name == "foreign" || name == "cistella"),
        "no sibling may be inscribed when the created session is gone: {owned:?}"
    );

    // The inscription call must use the captured id handle, never a name.
    let targets = set_option_targets(&tmux_log(&fake_tmux));
    assert!(!targets.is_empty(), "expected at least one set-option call");
    for target in &targets {
        assert!(
            target.starts_with('$'),
            "set-option must target a captured session id, got: {target}"
        );
    }

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
}

#[test]
fn failed_creation_issues_no_set_option() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_cistella_bundle(&config_root);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);
    fail_fake_tmux_session_creation(&fake_tmux, "cistella");

    let child = spawn_host_relay(&config_root, &state_root, &inscriptions_root, &fake_tmux);
    wait_for_relay_ready(&state_root, "alpha");

    let listed_json = list_bundle(&state_root, &inscriptions_root, &config_root);
    assert!(
        listed_json["bundle"]["startup_failure_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "expected a startup failure record for the refused creation: {listed_json}"
    );
    assert!(
        set_option_targets(&tmux_log(&fake_tmux)).is_empty(),
        "no set-option call may follow a failed creation"
    );
    assert!(
        read_fake_tmux_owned_sessions(&fake_tmux).is_empty(),
        "nothing may be inscribed when creation fails"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
}

#[test]
fn malformed_session_id_report_issues_no_set_option() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_cistella_bundle(&config_root);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);
    fail_fake_tmux_session_id_report(&fake_tmux, "cistella");

    let child = spawn_host_relay(&config_root, &state_root, &inscriptions_root, &fake_tmux);
    wait_for_relay_ready(&state_root, "alpha");

    let listed_json = list_bundle(&state_root, &inscriptions_root, &config_root);
    assert!(
        listed_json["bundle"]["startup_failure_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "expected a startup failure record for the malformed id: {listed_json}"
    );
    assert!(
        set_option_targets(&tmux_log(&fake_tmux)).is_empty(),
        "no set-option call may follow an unparseable session id"
    );
    assert!(
        read_fake_tmux_owned_sessions(&fake_tmux).is_empty(),
        "nothing may be inscribed without a valid captured id"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
}
