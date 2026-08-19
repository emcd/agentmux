use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
};

use serde_json::Value;
use tempfile::TempDir;

use super::super::support::process;
use super::helpers::*;

#[test]
fn up_requires_selector_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["up"])
        .output()
        .expect("run agentmux up");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid argument <bundle-id>|--group: missing selector"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn down_rejects_conflicting_bundle_and_group_selectors() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["down", "alpha", "--group", "dev"])
        .output()
        .expect("run agentmux down with conflicting selectors");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_selectors"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn up_and_down_report_idempotent_transitions() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
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

    let first_up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run first up");
    assert!(first_up.status.success(), "first up should succeed");
    let first_up_json = parse_summary_json_line(&first_up.stdout);
    assert_eq!(first_up_json["action"], "up");
    assert_eq!(first_up_json["changed_any"], true);
    assert_eq!(first_up_json["bundles"][0]["outcome"], "hosted");

    let second_up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run second up");
    assert!(second_up.status.success(), "second up should succeed");
    let second_up_json = parse_summary_json_line(&second_up.stdout);
    assert_eq!(second_up_json["changed_any"], false);
    assert_eq!(second_up_json["bundles"][0]["outcome"], "skipped");
    assert_eq!(
        second_up_json["bundles"][0]["reason_code"],
        "already_hosted"
    );

    let first_down = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run first down");
    assert!(first_down.status.success(), "first down should succeed");
    let first_down_json = parse_summary_json_line(&first_down.stdout);
    assert_eq!(first_down_json["action"], "down");
    assert_eq!(first_down_json["bundles"][0]["outcome"], "unhosted");

    let second_down = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run second down");
    assert!(second_down.status.success(), "second down should succeed");
    let second_down_json = parse_summary_json_line(&second_down.stdout);
    assert_eq!(second_down_json["bundles"][0]["outcome"], "skipped");
    assert_eq!(
        second_down_json["bundles"][0]["reason_code"],
        "already_unhosted"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let host_output = host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for relay host");
    assert!(host_output.status.success(), "host should succeed");
}

/// A relay host plus a bundle whose members the fake tmux can be told to leave
/// without a pane, so a session that exists but is not ready is produced without
/// depending on how fast anything exits.
struct BundleCliHarness {
    _temporary: TempDir,
    config_root: PathBuf,
    state_root: PathBuf,
    inscriptions_root: PathBuf,
    fake_tmux: PathBuf,
    host: Option<process::RelayChildGuard>,
}

impl BundleCliHarness {
    fn start(members: &[&str]) -> Self {
        Self::start_bundles(&[("alpha", members)], None)
    }

    /// Multi-bundle variant: each entry is a bundle name and its member session
    /// ids, and every bundle joins `groups` when one is given so a selector can
    /// reach them together. Member ids are keyed to the one shared fake tmux, so
    /// they must be distinct across bundles for the per-session failure controls
    /// to name one bundle's member.
    fn start_bundles(bundles: &[(&str, &[&str])], groups: Option<&[&str]>) -> Self {
        let temporary = TempDir::new().expect("temporary");
        let config_root = temporary.path().join("config");
        let state_root = temporary.path().join("state");
        let inscriptions_root = temporary.path().join("inscriptions");
        fs::create_dir_all(&config_root).expect("create config root");
        fs::create_dir_all(&state_root).expect("create state root");
        fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
        for (bundle_name, members) in bundles {
            write_bundle_configuration_with_options(
                &config_root,
                bundle_name,
                groups,
                members,
                Some(false),
            );
        }
        let fake_tmux = temporary.path().join("fake-tmux.sh");
        write_fake_tmux_script(&fake_tmux);

        let host = process::RelayChildGuard::new(
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

        Self {
            _temporary: temporary,
            config_root,
            state_root,
            inscriptions_root,
            fake_tmux,
            host: Some(host),
        }
    }

    fn run(&self, arguments: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args(arguments)
            .args([
                "--configuration-directory",
                &self.config_root.to_string_lossy(),
                "--state-directory",
                &self.state_root.to_string_lossy(),
                "--inscriptions-directory",
                &self.inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &self.fake_tmux)
            .output()
            .unwrap_or_else(|source| panic!("run agentmux {arguments:?}: {source}"))
    }
}

impl Drop for BundleCliHarness {
    fn drop(&mut self) {
        shutdown_relay_if_present(&self.state_root, "alpha");
        if let Some(host) = self.host.take() {
            let _ = host.wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT);
        }
    }
}

#[test]
fn up_reports_a_partially_started_bundle_as_degraded_and_names_the_failed_session() {
    let harness = BundleCliHarness::start(&["a", "u"]);
    // `u` is created like any other member; it just never yields a pane, so it
    // is the created-but-not-ready case rather than a creation failure.
    mark_fake_tmux_session_unready(&harness.fake_tmux, "u");

    let up = harness.run(&["up", "alpha"]);
    assert!(up.status.success(), "up should not fail the transition");
    let summary = parse_summary_json_line(&up.stdout);

    assert_eq!(summary["bundles"][0]["outcome"], "degraded");
    assert_eq!(summary["degraded_bundle_count"], 1);
    assert_eq!(summary["changed_bundle_count"], 0);
    assert_eq!(summary["failed_bundle_count"], 0);
    assert_eq!(summary["changed_any"], true);
    let failed_sessions = summary["bundles"][0]["details"]["failed_sessions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected failed_sessions detail: {summary}"));
    assert_eq!(failed_sessions.len(), 1, "unexpected detail: {summary}");
    assert_eq!(failed_sessions[0]["session_id"], "u");

    let stdout = String::from_utf8_lossy(&up.stdout);
    assert!(
        stdout.contains("session=u") && stdout.contains("degraded=1"),
        "up text output should name the failed session: {stdout}"
    );

    // The failure `up` reported has to be the one a following `list` reports, or
    // the two surfaces tell the operator different stories about one bundle.
    let listed = harness.run(&["list", "principals", "--namespace", "alpha", "--json"]);
    assert!(listed.status.success(), "list should succeed");
    let listed_json: Value = serde_json::from_slice(&listed.stdout).expect("decode list payload");
    let failures = listed_json["bundle"]["recent_startup_failures"]
        .as_array()
        .expect("startup failures array");
    assert!(
        failures.iter().any(|failure| failure["session_id"] == "u"),
        "up's recorded failure should reach list: {listed_json}"
    );

    // Nothing is created on the second pass, so an outcome derived from what
    // changed would report `skipped` here. It stays degraded because the outcome
    // comes from readiness, evaluated across members `up` did not create.
    let second_up = harness.run(&["up", "alpha"]);
    assert!(second_up.status.success(), "second up should succeed");
    let second_summary = parse_summary_json_line(&second_up.stdout);
    assert_eq!(second_summary["bundles"][0]["outcome"], "degraded");
    assert_eq!(second_summary["degraded_bundle_count"], 1);
}

#[test]
fn up_continues_reconciling_after_a_member_fails_to_be_created() {
    let harness = BundleCliHarness::start(&["a", "u"]);
    // `a` sorts first and is therefore the bootstrap member — the site that used
    // to propagate the first creation error and abandon every remaining member,
    // leaving the bundle partly up while reporting a total failure.
    fail_fake_tmux_session_creation(&harness.fake_tmux, "a");

    let up = harness.run(&["up", "alpha"]);
    assert!(up.status.success(), "up should not fail the transition");
    let summary = parse_summary_json_line(&up.stdout);

    assert_eq!(summary["bundles"][0]["outcome"], "degraded");
    let failed_sessions = summary["bundles"][0]["details"]["failed_sessions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected failed_sessions detail: {summary}"));
    assert_eq!(failed_sessions.len(), 1, "unexpected detail: {summary}");
    assert_eq!(failed_sessions[0]["session_id"], "a");
    assert_eq!(
        failed_sessions[0]["reason"],
        "failed to create tmux session during reconciliation"
    );

    // The member after the failed bootstrap must still have been attempted.
    let listed = harness.run(&["list", "principals", "--namespace", "alpha", "--json"]);
    let listed_json: Value = serde_json::from_slice(&listed.stdout).expect("decode list payload");
    let principals = listed_json["bundle"]["principals"]
        .as_array()
        .expect("principals array");
    let surviving = principals
        .iter()
        .find(|principal| principal["id"] == "u@alpha")
        .unwrap_or_else(|| panic!("expected surviving member in list: {listed_json}"));
    assert_eq!(
        surviving["ready"], true,
        "the member after the failed bootstrap should still be up: {listed_json}"
    );
}

#[test]
fn up_reports_a_bundle_with_no_ready_session_as_failed() {
    let harness = BundleCliHarness::start(&["a", "u"]);
    mark_fake_tmux_session_unready(&harness.fake_tmux, "a");
    mark_fake_tmux_session_unready(&harness.fake_tmux, "u");

    let up = harness.run(&["up", "alpha"]);
    // Parsed before the status is asserted, and deliberately: the payload has to
    // survive the failure. Answering for the failed count ahead of rendering
    // would leave the caller an exit code and nothing naming what failed.
    let summary = parse_summary_json_line(&up.stdout);

    // Both sessions were created; neither is ready. `degraded` would claim the
    // bundle has something serving, so the outcome has to be `failed`.
    assert_eq!(summary["bundles"][0]["outcome"], "failed");
    assert_eq!(summary["failed_bundle_count"], 1);
    assert_eq!(summary["degraded_bundle_count"], 0);
    assert_eq!(summary["changed_any"], false);
    let failed_sessions = summary["bundles"][0]["details"]["failed_sessions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected failed_sessions detail: {summary}"));
    assert_eq!(failed_sessions.len(), 2, "unexpected detail: {summary}");

    assert!(
        !up.status.success(),
        "up should exit non-zero when a bundle failed: {summary}"
    );
    let stderr = String::from_utf8_lossy(&up.stderr);
    assert!(
        stderr.contains("runtime_transition_failed"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn up_exits_non_zero_when_one_bundle_of_a_group_fails() {
    let harness =
        BundleCliHarness::start_bundles(&[("alpha", &["a"]), ("beta", &["b"])], Some(&["dev"]));
    // Only `beta`'s member is held unready, so the run genuinely mixes: `alpha`
    // comes up whole and `beta` has nothing serving.
    mark_fake_tmux_session_unready(&harness.fake_tmux, "b");

    let up = harness.run(&["up", "--group", "dev"]);
    let summary = parse_summary_json_line(&up.stdout);

    assert_eq!(summary["changed_bundle_count"], 1, "unexpected: {summary}");
    assert_eq!(summary["failed_bundle_count"], 1, "unexpected: {summary}");
    assert_eq!(summary["changed_any"], true, "unexpected: {summary}");

    // The bundle that came up must still be reported as hosted: a partial run
    // does not get rewritten into a total failure by the exit status.
    let outcomes: Vec<&str> = summary["bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("expected bundles array: {summary}"))
        .iter()
        .map(|bundle| bundle["outcome"].as_str().unwrap_or("<absent>"))
        .collect();
    assert!(
        outcomes.contains(&"hosted") && outcomes.contains(&"failed"),
        "expected a mixed run: {summary}"
    );

    // The threshold `host relay` uses -- fail only when nothing came up -- would
    // exit zero here. A transition command has no reason to keep running, so any
    // failed bundle fails the run.
    assert!(
        !up.status.success(),
        "up should exit non-zero when one bundle of the group failed: {summary}"
    );
}

#[test]
fn up_still_fails_whole_operation_for_an_error_no_single_session_owns() {
    let harness = BundleCliHarness::start(&["a", "u"]);
    // A tmux state query that fails is not attributable to one session, so the
    // per-session tolerance must not swallow it into a partial result.
    fail_fake_tmux_state_queries(&harness.fake_tmux);

    let up = harness.run(&["up", "alpha"]);
    assert!(
        !up.status.success(),
        "up should fail outright: {}",
        String::from_utf8_lossy(&up.stdout)
    );
    let stderr = String::from_utf8_lossy(&up.stderr);
    assert!(
        stderr.contains("internal_unexpected_failure"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn down_reports_relay_unavailable_when_relay_is_not_running() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .output()
        .expect("run down without relay");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("relay_unavailable"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn up_rejects_caller_whose_policy_lacks_updown() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
    write_tui_configuration(
        &config_root,
        None,
        Some("limited"),
        &[("limited", "default", Some("Limited"))],
    );
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

    let attempt = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run unauthorized up");
    assert!(
        !attempt.status.success(),
        "unauthorized up should fail; stdout={} stderr={}",
        String::from_utf8_lossy(&attempt.stdout),
        String::from_utf8_lossy(&attempt.stderr),
    );
    let stderr = String::from_utf8_lossy(&attempt.stderr);
    assert!(
        stderr.contains("authorization_forbidden"),
        "expected authorization_forbidden in stderr, got: {stderr}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let host_output = host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for relay host");
    assert!(host_output.status.success(), "host should succeed");
}

#[test]
fn up_succeeds_for_operator_policy_with_updown_capability() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
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

    let attempt = Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
        .expect("run operator up");
    assert!(
        attempt.status.success(),
        "operator up should succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&attempt.stdout),
        String::from_utf8_lossy(&attempt.stderr),
    );
    let summary = parse_summary_json_line(&attempt.stdout);
    assert_eq!(summary["action"], "up");
    assert_eq!(summary["bundles"][0]["outcome"], "hosted");

    shutdown_relay_if_present(&state_root, "alpha");
    let host_output = host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for relay host");
    assert!(host_output.status.success(), "host should succeed");
}

/// A `policies.toml` in an earlier layer must govern the decision the relay
/// actually enforces, not merely the presets `check` validates. The base layer
/// grants `updown` and the earlier layer withholds it, so a permitted outcome
/// here means authorization consulted a document the operator believed they had
/// replaced.
#[test]
fn an_earlier_policies_layer_governs_relay_authorization() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
    let override_layer = temporary.path().join("override");
    fs::create_dir_all(&override_layer).expect("create override layer");
    fs::write(
        override_layer.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
find = "self"
choose = "home"
list = "all"
look = "all"
raww = "home"
send = "all"
updown = "none"
"#,
    )
    .expect("write override policies config");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    // Two occurrences: the override layer first, so its policies.toml shadows
    // the base layer's while the bundle definition still resolves from the base.
    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--no-autostart",
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
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let attempt = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
            "alpha",
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
        .output()
        .expect("run up under layered policies");
    assert!(
        !attempt.status.success(),
        "an earlier layer withholding updown should forbid up; stdout={} stderr={}",
        String::from_utf8_lossy(&attempt.stdout),
        String::from_utf8_lossy(&attempt.stderr),
    );
    let stderr = String::from_utf8_lossy(&attempt.stderr);
    assert!(
        stderr.contains("authorization_forbidden"),
        "expected authorization_forbidden in stderr, got: {stderr}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let host_output = host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for relay host");
    assert!(host_output.status.success(), "host should succeed");
}

#[test]
fn host_relay_summary_json_omits_group_name() {
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
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary_line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{') && line.contains("\"host_mode\""))
        .expect("find startup summary json line");
    let payload: Value = serde_json::from_str(summary_line).expect("parse summary payload");
    let payload_object = payload.as_object().expect("summary payload object");
    assert_eq!(
        payload_object
            .get("host_mode")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "autostart"
    );
    assert!(
        !payload_object.contains_key("group_name"),
        "group_name should be omitted in single-bundle mode payload: {payload_object:?}"
    );
}
