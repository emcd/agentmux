use std::{path::PathBuf, time::Duration};

use agentmux::{
    configuration::ConfigurationRoots,
    relay::{RelayRequest, RelayResponse, SendOutcome, handle_request},
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

use crate::support::relay_delivery::{
    CoderSpec, SessionSpec, TmuxServerGuard, capture_pane, spawn_session, tmux_available,
    tmux_command, wait_for_pane_contains, write_bundle_configuration_members,
};

/// Negative-assertion budget: how long to wait after `SendOutcome::Queued`
/// before asserting the message has NOT been injected into the tmux pane.
///
/// What the wait proves is that the member is *held*, not that it was dropped —
/// a held member has no deadline, so no budget can establish "never", only "not
/// within a window many times longer than delivery takes when the gate opens".
/// The positive tests in this file deliver inside 3 seconds against the same
/// fixture, so 2 seconds of silence is not a delivery that was merely slow.
const RELAY_NEGATIVE_DELIVERY_BUDGET: Duration = Duration::from_millis(2_000);

fn dispatch_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    runtime_directory: &std::path::Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(request, configuration_roots, bundle_name, runtime_directory)
}

#[test]
fn relay_send_delivers_when_prompt_readiness_template_matches() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("READY>".to_string()),
                prompt_inspect_lines: Some(8),
                prompt_idle_column: None,
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "printf 'booting\\n'; sleep 0.2; printf 'READY>\\n'; exec sleep 45",
    );

    let marker = "PROMPT-TEMPLATE-MARKER";
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-ready".to_string()),
            requester_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(50),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("async send should be accepted");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        marker,
        Duration::from_millis(3_000),
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

/// A healthy target whose prompt-readiness template never matches holds its
/// member rather than delivering to it.
///
/// The invariant predates the relocation of the wait, but what enforces it does
/// not: Tmux used to own the prompt wait and withhold injection itself, and now
/// the transport reports an advisory level that relay admission gates on. The
/// observable contract is the same either way, which is the point — a member
/// whose target is reachable but not at a prompt is held, not written.
#[test]
fn relay_send_times_out_when_prompt_readiness_never_matches() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("^›".to_string()),
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "printf 'idle\\n'; exec sleep 45",
    );

    let marker = "PROMPT-NEVER-MARKER";
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-unready".to_string()),
            requester_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(50),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("async send should be accepted");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    std::thread::sleep(RELAY_NEGATIVE_DELIVERY_BUDGET);
    let snapshot = capture_pane(&paths.tmux_socket, "bravo", "-200");
    assert!(
        !snapshot.contains(marker),
        "message must not be injected while prompt readiness never matches, snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_send_delivers_when_prompt_idle_column_matches() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("(?m)^READY>".to_string()),
                prompt_inspect_lines: Some(3),
                prompt_idle_column: Some(6),
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "PS1='READY>'; export PS1; exec bash --noprofile --norc -i",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "READY>",
        Duration::from_millis(1_200),
    );

    let marker = "IDLE-COLUMN-MARKER";
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-idle-match".to_string()),
            requester_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("async send should be accepted");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        marker,
        Duration::from_millis(3_000),
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_send_delivers_when_prompt_regex_requires_blank_separator_line() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("(?ms)^READY>.*\\n\\nstatus.*$".to_string()),
                prompt_inspect_lines: Some(3),
                prompt_idle_column: None,
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "PS1='READY>\\n\\nstatus '; export PS1; exec bash --noprofile --norc -i",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "status",
        Duration::from_millis(1_200),
    );

    let marker = "BLANK-SEPARATOR-MARKER";
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-blank-line".to_string()),
            requester_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("async send should be accepted");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        marker,
        Duration::from_millis(3_000),
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

/// The same hold, reached through the idle-column half of the template rather
/// than the regex half. Kept separate because the two are independently
/// sufficient to make a pane unready, and a gate that consulted only one of them
/// would pass the other test.
#[test]
fn relay_send_times_out_when_prompt_idle_column_does_not_match() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("(?m)^READY>".to_string()),
                prompt_inspect_lines: Some(3),
                prompt_idle_column: Some(6),
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "PS1='READY>'; export PS1; exec bash --noprofile --norc -i",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "READY>",
        Duration::from_millis(1_200),
    );
    let typed = tmux_command(
        &paths.tmux_socket,
        &["send-keys", "-t", "bravo", "--", "echo hi"],
    );
    assert!(
        typed.status.success(),
        "failed to prefill prompt input: {}",
        String::from_utf8_lossy(&typed.stderr)
    );

    let marker = "IDLE-MISMATCH-MARKER";
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-idle-mismatch".to_string()),
            requester_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("async send should be accepted");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    std::thread::sleep(RELAY_NEGATIVE_DELIVERY_BUDGET);
    let snapshot = capture_pane(&paths.tmux_socket, "bravo", "-200");
    assert!(
        !snapshot.contains(marker),
        "message must not be injected while prompt idle column never matches, snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

/// The marker the Tmux transport prefixes onto a terminal-outcome receipt, which
/// is how a receipt to the sender is observable in a pane at all.
const RECEIPT_MARKER: &str = "--- agentmux terminal-outcome receipt ---";

/// Counts inscriptions of one event in the process log.
fn count_inscriptions(path: &std::path::Path, event: &str) -> usize {
    let needle = format!("\"event\":\"{event}\"");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(needle.as_str()))
        .count()
}

/// Waits for one inscription of `event`, returning its line.
fn await_inscription(path: &std::path::Path, event: &str, budget: Duration) -> String {
    let needle = format!("\"event\":\"{event}\"");
    let started = std::time::Instant::now();
    loop {
        let log = std::fs::read_to_string(path).unwrap_or_default();
        if let Some(line) = log.lines().find(|line| line.contains(needle.as_str())) {
            return line.to_string();
        }
        assert!(
            started.elapsed() < budget,
            "timed out waiting for {event}, log={log}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A member held on readiness waits without a bound, keeps its admission quota
/// while it waits, and is delivered when its target finally reaches a prompt.
///
/// The three claims belong in one test because separating them would let each
/// pass for the wrong reason. A hold that quietly released its quota would still
/// look like a hold from the pane; a hold that resolved the member would still
/// look like a hold from the quota. Only asserting them against the same held
/// member, and then watching that member deliver, distinguishes an indefinite
/// wait from a slow failure.
///
/// The wait is the contract, not an implementation detail: how long a target
/// stays busy is not evidence about the target, so no elapsed duration converts
/// this wait into an outcome. There is deliberately no configuration knob that
/// bounds it — `unreachable-dwell-ms` bounds continuous *unreachability*, a
/// different axis, and it is set high here so it cannot be what resolves
/// anything.
///
/// The quota probe is the load-bearing half of "releases its quota". A per-target
/// quota of one means a second send is refused exactly as long as the first
/// member still holds its slot; if elapsed time released the slot behind the
/// sender's back, the second send would be accepted instead.
#[test]
fn a_member_held_on_readiness_keeps_its_quota_and_delivers_when_the_prompt_arrives() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    if !tmux_available() {
        eprintln!("skipping readiness-hold test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    configure_delivery(DeliveryConfiguration {
        // One slot per target, so the second send can ask whether the first
        // member still holds its reservation.
        queued_envelopes_per_target_max: 1,
        // Far longer than this test runs. The target is reachable throughout, so
        // the dwell should never be consulted; setting it high means a passing
        // run cannot be one where an unreachable reading resolved the member on
        // the other axis.
        unreachable_dwell_ms: 600_000,
        ..DeliveryConfiguration::default()
    });

    let bundle_name = "party";
    let config_root = write_bundle_configuration_members(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "default".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: None,
                prompt_inspect_lines: None,
                prompt_idle_column: None,
            },
            CoderSpec {
                id: "prompt".to_string(),
                initial_command: "sh -lc 'exec sleep 45'".to_string(),
                resume_command: "sh -lc 'exec sleep 45'".to_string(),
                prompt_regex: Some("READY>".to_string()),
                prompt_inspect_lines: Some(8),
                prompt_idle_column: None,
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: Some("alpha".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "default".to_string(),
                coder_session_id: None,
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: Some("bravo".to_string()),
                directory: PathBuf::from("/tmp"),
                coder: "prompt".to_string(),
                coder_session_id: None,
            },
        ],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    // The pane reaches its prompt only when this file appears, which is what puts
    // the moment the gate opens under the test's control rather than a timer's.
    let release = temporary.path().join("reach-the-prompt");
    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        &format!(
            "printf 'idle\\n'; while [ ! -f {} ]; do sleep 0.1; done; printf 'READY>\\n'; exec sleep 45",
            release.display()
        ),
    );

    let marker = "READINESS-HOLD-MARKER";
    let send = |request_id: &str| {
        dispatch_request(
            RelayRequest::Send {
                request_id: Some(request_id.to_string()),
                requester_session: "alpha".to_string(),
                message: marker.to_string(),
                targets: vec!["bravo@party".to_string()],
                broadcast: false,
                quiet_window_ms: Some(50),
                on_behalf_of: None,
            },
            &config_root,
            bundle_name,
            &paths.runtime_directory,
        )
    };

    let response = send("req-held").expect("async send should be accepted");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    std::thread::sleep(RELAY_NEGATIVE_DELIVERY_BUDGET);

    let snapshot = capture_pane(&paths.tmux_socket, "bravo", "-200");
    assert!(
        !snapshot.contains(marker),
        "a member held on readiness must not be written, snapshot={snapshot:?}"
    );
    assert_eq!(
        count_inscriptions(&inscriptions, "relay.send.async.completed"),
        0,
        "no elapsed duration resolves a held member whose target is reachable"
    );
    let sender_pane = capture_pane(&paths.tmux_socket, "alpha", "-200");
    assert!(
        !sender_pane.contains(RECEIPT_MARKER),
        "a held member produces no receipt to its sender, snapshot={sender_pane:?}"
    );

    // The quota probe. Refusal here means the held member still owns its slot.
    let refusal = send("req-second").expect_err("the held member still holds the only slot");
    assert_eq!(refusal.code, "runtime_delivery_queue_full");

    // Open the gate. Everything above was the wait; this is the delivery it was
    // waiting for, and without it the assertions above would be equally true of a
    // member that was silently dropped.
    std::fs::write(&release, b"").expect("release the prompt");

    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        marker,
        Duration::from_millis(10_000),
    );
    let completed = await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        Duration::from_millis(10_000),
    );
    let record: serde_json::Value =
        serde_json::from_str(completed.as_str()).expect("completed inscription is json");
    assert_eq!(
        record["details"]["outcome"].as_str(),
        Some("delivered"),
        "the member that waited is the one that delivers: {completed}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}
