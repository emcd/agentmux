use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use agentmux::relay::{RelayError, RelayResponse};
use agentmux::runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory};
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn host_relay_rejects_positional_bundle_selector() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "alpha"])
        .output()
        .expect("run agentmux host relay");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_arguments"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_rejects_group_selector_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--group", "dev"])
        .output()
        .expect("run agentmux host relay with group selector");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--group") && stderr.contains("unknown argument"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_rejects_all_flag_in_group_mvp() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--all"])
        .output()
        .expect("run agentmux host relay --all");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_arguments"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_default_mode_starts_autostart_bundles() {
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
    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(false),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_socket(&state_root, "alpha");
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output()
        .expect("wait for agentmux host relay");

    assert!(output.status.success(), "command should succeed");
    let summary_json = parse_summary_json_line(&output.stdout);
    let bundles = summary_json["bundles"]
        .as_array()
        .expect("startup summary bundles");
    let alpha = bundles
        .iter()
        .find(|bundle| bundle["bundle_name"] == "alpha")
        .expect("alpha startup summary");
    let bravo = bundles
        .iter()
        .find(|bundle| bundle["bundle_name"] == "bravo")
        .expect("bravo startup summary");
    assert!(
        summary_json["host_mode"] == "autostart",
        "unexpected summary: {summary_json}"
    );
    assert!(
        summary_json["hosted_bundle_count"] == 1
            && summary_json["skipped_bundle_count"] == 1
            && summary_json["failed_bundle_count"] == 0
            && summary_json["hosted_any"] == true,
        "unexpected summary: {summary_json}"
    );
    assert_eq!(alpha["outcome"], "hosted");
    assert_eq!(bravo["outcome"], "skipped");
    assert_eq!(bravo["reason_code"], "process_only");
}

#[test]
fn host_relay_no_autostart_mode_reports_process_only_summary() {
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

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
            "--config-directory",
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
        .expect("spawn agentmux host relay --no-autostart");
    wait_for_relay_socket(&state_root, "alpha");
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output()
        .expect("wait for agentmux host relay --no-autostart");

    assert!(output.status.success(), "command should succeed");
    let summary_json = parse_summary_json_line(&output.stdout);
    let bundles = summary_json["bundles"]
        .as_array()
        .expect("startup summary bundles");
    let alpha = bundles
        .iter()
        .find(|bundle| bundle["bundle_name"] == "alpha")
        .expect("alpha startup summary");
    assert!(
        summary_json["host_mode"] == "process_only",
        "unexpected summary: {summary_json}"
    );
    assert!(
        summary_json["hosted_bundle_count"] == 0
            && summary_json["skipped_bundle_count"] == 1
            && summary_json["failed_bundle_count"] == 0
            && summary_json["hosted_any"] == false,
        "unexpected summary: {summary_json}"
    );
    assert_eq!(alpha["outcome"], "skipped");
    assert_eq!(alpha["reason_code"], "process_only");
}

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

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
            "--config-directory",
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
        .expect("spawn agentmux host relay --no-autostart");
    wait_for_relay_socket(&state_root, "alpha");

    let first_up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
            "alpha",
            "--config-directory",
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
            "--config-directory",
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
            "--config-directory",
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
            "--config-directory",
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
    let host_output = host_child.wait_with_output().expect("wait for relay host");
    assert!(host_output.status.success(), "host should succeed");
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
            "--config-directory",
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

    let child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--config-directory",
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
        .expect("spawn agentmux host relay");
    wait_for_relay_socket(&state_root, "alpha");
    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output()
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

#[test]
fn send_rejects_missing_message_input() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["send", "--target", "bravo"])
        .stdin(Stdio::null())
        .output()
        .expect("run agentmux send without message");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_missing_message_input"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_rejects_conflicting_flag_and_piped_message_sources() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["send", "--target", "bravo", "--message", "hello"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin
            .write_all(b"hello from stdin")
            .expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_message_input"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_rejects_conflicting_timeout_flags() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--target",
            "bravo",
            "--quiescence-timeout-ms",
            "1000",
            "--acp-turn-timeout-ms",
            "2000",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send with conflicting timeout flags");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin
            .write_all(b"hello from stdin")
            .expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_timeout_fields"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_rejects_invalid_acp_turn_timeout_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--target",
            "bravo",
            "--message",
            "hello",
            "--acp-turn-timeout-ms",
            "0",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run agentmux send with invalid ACP timeout");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_acp_turn_timeout"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_preserves_valid_explicit_sender_in_relay_request() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &["alpha", "bravo"],
    );

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Chat {
            schema_version: "1".to_string(),
            bundle_name: "agentmux".to_string(),
            request_id: None,
            sender_session: "alpha".to_string(),
            sender_display_name: Some("Alpha".to_string()),
            delivery_mode: agentmux::relay::ChatDeliveryMode::Async,
            status: agentmux::relay::ChatStatus::Accepted,
            results: vec![agentmux::relay::ChatResult {
                target_session: "bravo".to_string(),
                message_id: "msg-1".to_string(),
                outcome: agentmux::relay::ChatOutcome::Queued,
                reason_code: None,
                reason: None,
                details: None,
            }],
        },
        Arc::clone(&request_log),
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--bundle",
            "agentmux",
            "--sender",
            "alpha",
            "--target",
            "bravo",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin.write_all(b"hello").expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    relay_thread.join().expect("join fake relay thread");

    assert!(output.status.success(), "command should succeed");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode send json payload");
    assert_eq!(payload["sender_session"], "alpha");

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "chat");
    assert_eq!(requests[0]["sender_session"], "alpha");
}

#[test]
fn send_rejects_unknown_explicit_sender_without_fallback() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    let workspace_root = temporary.path().join("workspace");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    fs::create_dir_all(&workspace_root).expect("create workspace root");
    write_bundle_configuration_with_member_directories(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &[
            ("alpha", workspace_root.as_path()),
            ("bravo", Path::new("/tmp")),
        ],
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "send",
            "--bundle",
            "agentmux",
            "--sender",
            "ghost",
            "--target",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send with unknown sender");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin.write_all(b"hello").expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_sender"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stderr.contains("relay_unavailable"),
        "explicit sender should fail before relay transport fallback: {stderr}"
    );
}

#[test]
fn look_returns_canonical_json_payload() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "bravo"]);

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let workspace_root = temporary.path().join("workspace");
    configure_local_mcp_override(&workspace_root, "agentmux", "tui");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Look {
            schema_version: "1".to_string(),
            bundle_name: "agentmux".to_string(),
            requester_session: "tui".to_string(),
            target_session: "bravo".to_string(),
            captured_at: "2026-03-08T00:00:00Z".to_string(),
            snapshot_lines: vec!["LOOK-A".to_string(), "LOOK-B".to_string()],
        },
        Arc::clone(&request_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "look",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look");
    relay_thread.join().expect("join fake relay thread");

    assert!(output.status.success(), "command should succeed");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode look json payload");
    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["bundle_name"], "agentmux");
    assert_eq!(payload["requester_session"], "tui");
    assert_eq!(payload["target_session"], "bravo");
    assert_eq!(payload["captured_at"], "2026-03-08T00:00:00Z");
    assert_eq!(
        payload["snapshot_lines"],
        serde_json::json!(["LOOK-A", "LOOK-B"])
    );

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "look");
    assert_eq!(requests[0]["requester_session"], "tui");
    assert_eq!(requests[0]["target_session"], "bravo");
}

#[test]
fn look_rejects_cross_bundle_request_in_mvp() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    let workspace_root = temporary.path().join("workspace");
    configure_local_mcp_override(&workspace_root, "agentmux", "tui");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "look",
            "bravo",
            "--bundle",
            "other-bundle",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look --bundle");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_cross_bundle_unsupported"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn look_surfaces_authorization_forbidden_from_relay() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "bravo"]);
    let workspace_root = temporary.path().join("workspace");
    configure_local_mcp_override(&workspace_root, "agentmux", "tui");

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Error {
            error: RelayError {
                code: "authorization_forbidden".to_string(),
                message: "request denied by authorization policy".to_string(),
                details: Some(serde_json::json!({
                    "capability": "look.inspect",
                    "requester_session": "tui",
                    "bundle_name": "agentmux",
                    "target_session": "bravo",
                    "reason": "look policy scope permits self-only inspection",
                })),
            },
        },
        Arc::new(Mutex::new(Vec::<Value>::new())),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "look",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look");
    relay_thread.join().expect("join fake relay thread");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("authorization_forbidden"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn look_surfaces_unsupported_transport_from_relay() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "bravo"]);
    let workspace_root = temporary.path().join("workspace");
    configure_local_mcp_override(&workspace_root, "agentmux", "tui");

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Error {
            error: RelayError {
                code: "validation_unsupported_transport".to_string(),
                message: "look is unsupported for ACP targets in MVP".to_string(),
                details: Some(serde_json::json!({
                    "target_session": "bravo",
                    "transport": "acp",
                })),
            },
        },
        Arc::new(Mutex::new(Vec::<Value>::new())),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "look",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look");
    relay_thread.join().expect("join fake relay thread");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unsupported_transport"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn look_rejects_invalid_lines_bounds() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["look", "bravo", "--lines", "0"])
        .output()
        .expect("run agentmux look --lines 0");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_lines"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn unified_host_help_output_includes_relay_and_mcp_modes() {
    let relay = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--help"])
        .output()
        .expect("run agentmux host relay --help");
    assert!(relay.status.success(), "relay help should succeed");
    let relay_stdout = String::from_utf8_lossy(&relay.stdout);
    assert!(
        relay_stdout.contains("Usage: agentmux host relay"),
        "unexpected relay help output: {relay_stdout}"
    );
    assert!(
        !relay_stdout.contains("--group GROUP"),
        "unexpected relay help output: {relay_stdout}"
    );
    assert!(
        relay_stdout.contains("--no-autostart"),
        "unexpected relay help output: {relay_stdout}"
    );

    let mcp = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "mcp", "--help"])
        .output()
        .expect("run agentmux host mcp --help");
    assert!(mcp.status.success(), "mcp help should succeed");
    let mcp_stdout = String::from_utf8_lossy(&mcp.stdout);
    assert!(
        mcp_stdout.contains("Usage: agentmux host mcp"),
        "unexpected mcp help output: {mcp_stdout}"
    );
}

#[test]
fn tui_help_output_includes_usage_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["tui", "--help"])
        .output()
        .expect("run agentmux tui --help");
    assert!(output.status.success(), "tui help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: agentmux tui"),
        "unexpected tui help output: {stdout}"
    );
    assert!(
        stdout.contains("--bundle NAME"),
        "unexpected tui help output: {stdout}"
    );
}

#[test]
fn bare_agentmux_without_tty_prints_help_and_fails() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .output()
        .expect("run bare agentmux");
    assert!(
        !output.status.success(),
        "bare command should fail without tty"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Usage: agentmux <command>"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        stderr.contains("validation_missing_subcommand"),
        "unexpected stderr: {stderr}"
    );
}

fn write_bundle_configuration(
    config_root: &Path,
    bundle_name: &str,
    groups: Option<&[&str]>,
    sessions: &[&str],
) {
    write_bundle_configuration_with_options(config_root, bundle_name, groups, sessions, None);
}

fn write_bundle_configuration_with_options(
    config_root: &Path,
    bundle_name: &str,
    groups: Option<&[&str]>,
    sessions: &[&str],
    autostart: Option<bool>,
) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "self"
send = "all:home"
"#,
    )
    .expect("write policies config");
    let mut bundle = String::from("format-version = 1\n");
    if let Some(autostart) = autostart {
        bundle.push_str(format!("autostart = {autostart}\n").as_str());
    }
    if let Some(groups) = groups {
        let encoded = groups
            .iter()
            .map(|group| format!("\"{group}\""))
            .collect::<Vec<_>>()
            .join(", ");
        bundle.push_str(format!("groups = [{encoded}]\n").as_str());
    }
    for session in sessions {
        bundle.push_str(
            format!(
                "\n[[sessions]]\nid = \"{name}\"\nname = \"{name}\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n",
                name = session
            )
            .as_str(),
        );
    }
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        bundle,
    )
    .expect("write bundle config");
}

fn write_bundle_configuration_with_member_directories(
    config_root: &Path,
    bundle_name: &str,
    groups: Option<&[&str]>,
    members: &[(&str, &Path)],
) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "self"
send = "all:home"
"#,
    )
    .expect("write policies config");
    let mut bundle = String::from("format-version = 1\n");
    if let Some(groups) = groups {
        let encoded = groups
            .iter()
            .map(|group| format!("\"{group}\""))
            .collect::<Vec<_>>()
            .join(", ");
        bundle.push_str(format!("groups = [{encoded}]\n").as_str());
    }
    for (session, directory) in members {
        bundle.push_str(
            format!(
                "\n[[sessions]]\nid = \"{name}\"\nname = \"{name}\"\ndirectory = \"{}\"\ncoder = \"default\"\n",
                directory.display(),
                name = session
            )
            .as_str(),
        );
    }
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        bundle,
    )
    .expect("write bundle config");
}

fn parse_summary_json_line(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .expect("find summary json line");
    serde_json::from_str(line).expect("parse summary json")
}

fn write_fake_tmux_script(path: &Path) {
    let sessions_file = path.with_extension("sessions");
    let owned_file = path.with_extension("owned");
    let body = format!(
        r##"#!/usr/bin/env bash
set -euo pipefail

SESSIONS_FILE="{sessions}"
OWNED_FILE="{owned}"
touch "${{SESSIONS_FILE}}" "${{OWNED_FILE}}"

args=("$@")
if [[ "${{#args[@]}}" -ge 2 && "${{args[0]}}" == "-S" ]]; then
  args=("${{args[@]:2}}")
fi
command_name="${{args[0]-}}"

case "${{command_name}}" in
  has-session)
    target="${{args[2]#=}}"
    if [[ -s "${{SESSIONS_FILE}}" ]] && grep -Fxq "${{target}}" "${{SESSIONS_FILE}}"; then
      exit 0
    fi
    echo "can't find session: ${{target}}" >&2
    exit 1
    ;;
  list-sessions)
    if [[ ! -s "${{SESSIONS_FILE}}" ]]; then
      echo "no server running on /tmp/agentmux-fake" >&2
      exit 1
    fi
    format="${{args[2]-}}"
    owned_format=$'#{{session_name}}\t#{{@agentmux_owned}}'
    while IFS= read -r session; do
      [[ -z "${{session}}" ]] && continue
      if [[ "${{format}}" == "${{owned_format}}" || "${{format}}" == "#{{session_name}}\\t#{{@agentmux_owned}}" ]]; then
        marker=""
        if [[ -s "${{OWNED_FILE}}" ]] && grep -Fxq "${{session}}" "${{OWNED_FILE}}"; then
          marker="1"
        fi
        printf "%s\t%s\n" "${{session}}" "${{marker}}"
      else
        printf "%s\n" "${{session}}"
      fi
    done < "${{SESSIONS_FILE}}"
    ;;
  new-session)
    session_name="${{args[3]}}"
    printf "%s\n" "${{session_name}}" >> "${{SESSIONS_FILE}}"
    sort -u "${{SESSIONS_FILE}}" -o "${{SESSIONS_FILE}}"
    ;;
  set-option)
    session_name="${{args[2]#=}}"
    printf "%s\n" "${{session_name}}" >> "${{OWNED_FILE}}"
    sort -u "${{OWNED_FILE}}" -o "${{OWNED_FILE}}"
    ;;
  kill-session)
    session_name="${{args[2]#=}}"
    grep -Fxv "${{session_name}}" "${{SESSIONS_FILE}}" > "${{SESSIONS_FILE}}.tmp" || true
    mv "${{SESSIONS_FILE}}.tmp" "${{SESSIONS_FILE}}"
    grep -Fxv "${{session_name}}" "${{OWNED_FILE}}" > "${{OWNED_FILE}}.tmp" || true
    mv "${{OWNED_FILE}}.tmp" "${{OWNED_FILE}}"
    ;;
  kill-server)
    if [[ ! -s "${{SESSIONS_FILE}}" ]]; then
      echo "no server running on /tmp/agentmux-fake" >&2
      exit 1
    fi
    : > "${{SESSIONS_FILE}}"
    : > "${{OWNED_FILE}}"
    ;;
  *)
    :
    ;;
esac
"##,
        sessions = sessions_file.display(),
        owned = owned_file.display(),
    );
    fs::write(path, body).expect("write fake tmux script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("set fake tmux executable");
}

fn shutdown_relay_if_present(state_root: &Path, bundle_name: &str) {
    let paths = BundleRuntimePaths::resolve(state_root, bundle_name).expect("bundle paths");
    let Some(pid) = fs::read_to_string(&paths.relay_lock_file)
        .ok()
        .and_then(|value| value.lines().next().map(str::to_string))
        .and_then(|value| value.trim().parse::<i32>().ok())
    else {
        return;
    };
    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !paths.relay_socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_relay_socket(state_root: &Path, bundle_name: &str) {
    let paths = BundleRuntimePaths::resolve(state_root, bundle_name).expect("bundle paths");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if paths.relay_socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for relay socket {}",
        paths.relay_socket.display()
    );
}

fn spawn_fake_relay_once(
    socket_path: &Path,
    response: RelayResponse,
    request_log: Arc<Mutex<Vec<Value>>>,
) -> thread::JoinHandle<()> {
    if socket_path.exists() {
        fs::remove_file(socket_path).expect("remove stale relay socket");
    }
    let parent = socket_path.parent().expect("relay socket parent");
    fs::create_dir_all(parent).expect("create relay socket parent");
    let listener = UnixListener::bind(socket_path).expect("bind fake relay socket");
    listener
        .set_nonblocking(true)
        .expect("set fake relay listener nonblocking");
    let socket_path = socket_path.to_path_buf();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _address)) => {
                    let mut request_line = String::new();
                    let mut reader =
                        BufReader::new(stream.try_clone().expect("clone fake relay stream"));
                    reader
                        .read_line(&mut request_line)
                        .expect("read fake relay request");
                    let request: Value =
                        serde_json::from_str(request_line.trim_end()).expect("decode request");
                    request_log.lock().expect("request log lock").push(request);
                    let encoded =
                        serde_json::to_string(&response).expect("encode fake relay response");
                    stream
                        .write_all(encoded.as_bytes())
                        .expect("write fake relay response");
                    stream.write_all(b"\n").expect("write fake relay newline");
                    stream.flush().expect("flush fake relay response");
                    let _ = fs::remove_file(socket_path);
                    return;
                }
                Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(source) => panic!("accept fake relay connection: {source}"),
            }
        }
        panic!("timed out waiting for fake relay request");
    })
}

fn configure_local_mcp_override(workspace_root: &Path, bundle_name: &str, session_name: &str) {
    let override_path = workspace_root.join(".auxiliary/configuration/agentmux/overrides");
    fs::create_dir_all(&override_path).expect("create local override directory");
    let override_file = override_path.join("mcp.toml");
    let content = format!("bundle_name = \"{bundle_name}\"\nsession_name = \"{session_name}\"\n");
    fs::write(override_file, content).expect("write local mcp override file");
}
