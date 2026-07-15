use std::time::Duration;

use agentmux::{
    relay::{RelayRequest, RelayResponse, SendOutcome, handle_request},
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

use crate::support::relay_delivery::{
    TmuxServerGuard, capture_pane, spawn_session, tmux_available, tmux_command,
    wait_for_pane_contains, write_bundle_configuration,
};

fn dispatch_request(
    request: RelayRequest,
    configuration_root: &std::path::Path,
    bundle_name: &str,
    runtime_directory: &std::path::Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(request, configuration_root, bundle_name, runtime_directory)
}

#[test]
fn relay_send_async_processes_repeated_target_messages_in_fifo_order() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");

    let first_marker = "FIFO-ONE-MARKER";
    let second_marker = "FIFO-TWO-MARKER";

    let first = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-fifo-1".to_string()),
            requester_session: "alpha".to_string(),
            message: first_marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("first async send should be accepted");
    let RelayResponse::Send {
        results: first_results,
        ..
    } = first
    else {
        panic!("expected send response");
    };
    assert_eq!(first_results.len(), 1);
    assert_eq!(first_results[0].outcome, SendOutcome::Queued);

    let second = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-fifo-2".to_string()),
            requester_session: "alpha".to_string(),
            message: second_marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("second async send should be accepted");
    let RelayResponse::Send {
        results: second_results,
        ..
    } = second
    else {
        panic!("expected send response");
    };
    assert_eq!(second_results.len(), 1);
    assert_eq!(second_results[0].outcome, SendOutcome::Queued);

    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        first_marker,
        Duration::from_millis(2_000),
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        second_marker,
        Duration::from_millis(2_000),
    );

    let snapshot = capture_pane(&paths.tmux_socket, "bravo", "-200");
    let first_index = snapshot
        .find(first_marker)
        .expect("first marker should exist in pane");
    let second_index = snapshot
        .find(second_marker)
        .expect("second marker should exist in pane");
    assert!(
        first_index < second_index,
        "expected FIFO marker order, snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

#[test]
fn relay_send_async_without_timeout_waits_for_late_quiescence() {
    if !tmux_available() {
        eprintln!("skipping relay delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "i=0; while [ \"$i\" -lt 30 ]; do printf '\\rWORK-%02d' \"$i\"; i=$((i+1)); sleep 0.02; done; printf '\\nIDLE\\n'; exec sleep 45",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "WORK-",
        Duration::from_millis(1_200),
    );

    let marker = "ASYNC-LATE-QUIESCENCE-MARKER";
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-async-default".to_string()),
            requester_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(120),
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

/// A pane in tmux copy-mode (where a mouse-wheel scroll leaves it) must
/// still receive both the delivered body and the submit. tmux routes
/// `send-keys` through the copy-mode key table, which swallows an Enter
/// keypress, but writes a `paste-buffer` straight to the pane pty — so the
/// body and the submit (an unbracketed carriage return) both reach the
/// child while the pane stays in copy-mode, leaving the operator's scroll
/// position undisturbed. The child echoes each submitted line back wrapped
/// in `ECHOED[...]`; that wrapper appears only once a carriage return
/// crosses copy-mode and completes the child's `read`, so this test fails
/// if the submit is delivered as `send-keys Enter` instead of a paste.
#[test]
fn relay_raww_submits_through_copy_mode_pane() {
    if !tmux_available() {
        eprintln!("skipping copy-mode delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    // A child that reads a line from its tty and echoes it back wrapped, so
    // the wrapper is observable only after a carriage return submits the line.
    spawn_session(
        &paths.tmux_socket,
        "alpha",
        "while IFS= read -r line; do printf 'ECHOED[%s]\\n' \"$line\"; done",
    );

    // Put the pane into copy-mode, as a scroll-back would.
    let enter = tmux_command(&paths.tmux_socket, &["copy-mode", "-t", "alpha"]);
    assert!(
        enter.status.success(),
        "failed to enter copy-mode: {}",
        String::from_utf8_lossy(&enter.stderr)
    );

    let marker = "COPYMODE_SUBMIT_MARKER";
    let response = dispatch_request(
        RelayRequest::Raww {
            request_id: Some("req-copymode-submit".to_string()),
            requester_session: "alpha".to_string(),
            target_session: "alpha@party".to_string(),
            text: marker.to_string(),
            no_enter: false,
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("raww request should be accepted");
    let RelayResponse::Raww { status, .. } = response else {
        panic!("expected raww response");
    };
    assert_eq!(status, "queued");

    // The wrapper line appears only if the body AND the carriage return both
    // crossed copy-mode and the child's `read` completed.
    wait_for_pane_contains(
        &paths.tmux_socket,
        "alpha",
        &format!("ECHOED[{marker}]"),
        Duration::from_secs(5),
    );

    // The paste-buffer submit must not have knocked the pane out of
    // copy-mode: the operator's scroll position survives delivery.
    let in_mode = tmux_command(
        &paths.tmux_socket,
        &["display-message", "-p", "-t", "alpha", "#{pane_in_mode}"],
    );
    assert_eq!(
        String::from_utf8_lossy(&in_mode.stdout).trim(),
        "1",
        "pane must remain in copy-mode after delivery, stderr={}",
        String::from_utf8_lossy(&in_mode.stderr)
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}
