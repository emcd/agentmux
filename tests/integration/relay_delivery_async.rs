use std::time::Duration;

use agentmux::{
    relay::{RelayRequest, RelayResponse, SendOutcome, handle_request},
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

use std::path::PathBuf;

use crate::support::relay_delivery::{
    CoderSpec, SessionSpec, TmuxServerGuard, capture_pane, spawn_session, tmux_available,
    tmux_command, wait_for_pane_contains, write_bundle_configuration,
    write_bundle_configuration_members,
};

fn dispatch_request(
    request: RelayRequest,
    configuration_root: &std::path::Path,
    bundle_name: &str,
    runtime_directory: &std::path::Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(request, configuration_root, bundle_name, runtime_directory)
}

/// A non-delivered terminal outcome for a queued message must reach the original
/// coder-session sender through a receipt
/// delivered to its own transport — its tmux pane — WITHOUT the sender having to
/// read `relay.log`.
///
/// `alpha` is a responsive (unbounded-wait) sleep pane; `bravo`'s coder sets a
/// small `prime-timeout-ms` and its quiet pane produces no output, so a send to
/// `bravo` resolves as `Timeout`, a non-delivered outcome. The relay routes a
/// terminal-outcome receipt back to `alpha`'s live delivery worker, which renders
/// it into `alpha`'s pane. `alpha`'s worker is primed first by delivering one
/// message TO it (`bravo -> alpha`), since a receipt routes only to an
/// already-live sender worker and is otherwise dropped.
#[test]
fn relay_send_async_delivers_terminal_outcome_receipt_to_coder_sender() {
    if !tmux_available() {
        eprintln!("skipping terminal-outcome-receipt test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    // Two coders: `responsive` has no prompt gate, so it delivers to a quiet pane
    // (used to prime alpha's worker). `wedging` gates delivery on a prompt regex
    // that never appears in its quiet pane and bounds the wait with a prime
    // timeout, so a send to it resolves to a non-delivered terminal outcome
    // (Timeout, or Wedged first) rather than waiting unbounded.
    let coders = vec![
        CoderSpec::sleeping("responsive"),
        CoderSpec {
            prompt_regex: Some("READY_PROMPT_THAT_NEVER_APPEARS>".to_string()),
            prompt_inspect_lines: Some(3),
            prime_timeout_ms: Some(300),
            ..CoderSpec::sleeping("wedging")
        },
    ];
    let sessions = vec![
        SessionSpec {
            id: "alpha".to_string(),
            name: Some("alpha".to_string()),
            directory: PathBuf::from("/tmp"),
            coder: "responsive".to_string(),
            coder_session_id: None,
        },
        SessionSpec {
            id: "bravo".to_string(),
            name: Some("bravo".to_string()),
            directory: PathBuf::from("/tmp"),
            coder: "wedging".to_string(),
            coder_session_id: None,
        },
    ];
    let config_root =
        write_bundle_configuration_members(temporary.path(), bundle_name, &coders, &sessions);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");

    // Prime alpha's delivery worker by delivering one message TO alpha, so a
    // receipt has a live worker to route to.
    let prime_marker = "PRIME-ALPHA-WORKER-MARKER";
    dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-prime".to_string()),
            requester_session: "bravo".to_string(),
            message: prime_marker.to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("priming send should be accepted");
    wait_for_pane_contains(
        &paths.tmux_socket,
        "alpha",
        prime_marker,
        Duration::from_millis(3_000),
    );

    // alpha sends to bravo; bravo's quiet pane + prime-timeout resolves the
    // delivery as Timeout, a non-delivered terminal outcome.
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-to-slow".to_string()),
            requester_session: "alpha".to_string(),
            message: "PAYLOAD-FOR-UNRESPONSIVE-TARGET".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("send to the slow target should be accepted");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);
    let queued_message_id = results[0].message_id.clone();

    // The terminal-outcome receipt must land in ALPHA's pane — proving the sender
    // learns the outcome through its own transport, not by reading relay.log. Wait
    // on the relay/system `From` identity, a short needle that does not wrap.
    wait_for_pane_contains(
        &paths.tmux_socket,
        "alpha",
        "relay@RELAY",
        Duration::from_secs(8),
    );
    // tmux hard-wraps the pane at its column width, so flatten newlines before
    // matching the receipt's longer fields.
    let snapshot = capture_pane(&paths.tmux_socket, "alpha", "-200");
    let flattened = snapshot.replace('\n', "");
    assert!(
        flattened.contains("was not delivered"),
        "expected a non-delivered receipt in alpha's pane, snapshot={snapshot:?}"
    );
    assert!(
        flattened.contains("bravo@party"),
        "receipt should name the delivery target, snapshot={snapshot:?}"
    );
    assert!(
        flattened.contains(&queued_message_id),
        "receipt should name the queued message id, snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

/// Best-effort boundary: when the sender is not routable — it has no live
/// delivery worker — its receipt is dropped, not persisted or retried, and never
/// resurfaces once the sender does become routable.
///
/// `alpha` sends to the wedging target WITHOUT being primed, so it has no worker
/// when the non-delivered outcome resolves and the receipt is dropped. A fixed
/// wait past the target's prime timeout guarantees the drop happened while
/// `alpha` was unroutable. `alpha` is then primed and a sentinel delivered; the
/// dropped receipt must not reappear ahead of the sentinel.
#[test]
fn relay_send_async_drops_terminal_receipt_when_sender_not_routable() {
    if !tmux_available() {
        eprintln!("skipping receipt-drop test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let coders = vec![
        CoderSpec::sleeping("responsive"),
        CoderSpec {
            prompt_regex: Some("READY_PROMPT_THAT_NEVER_APPEARS>".to_string()),
            prompt_inspect_lines: Some(3),
            prime_timeout_ms: Some(300),
            ..CoderSpec::sleeping("wedging")
        },
    ];
    let sessions = vec![
        SessionSpec {
            id: "alpha".to_string(),
            name: Some("alpha".to_string()),
            directory: PathBuf::from("/tmp"),
            coder: "responsive".to_string(),
            coder_session_id: None,
        },
        SessionSpec {
            id: "bravo".to_string(),
            name: Some("bravo".to_string()),
            directory: PathBuf::from("/tmp"),
            coder: "wedging".to_string(),
            coder_session_id: None,
        },
    ];
    let config_root =
        write_bundle_configuration_members(temporary.path(), bundle_name, &coders, &sessions);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");

    // alpha sends to bravo while alpha has NO live worker: the non-delivered
    // outcome's receipt is dropped rather than spawning a worker for alpha.
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-unroutable".to_string()),
            requester_session: "alpha".to_string(),
            message: "PAYLOAD-WHILE-SENDER-UNROUTABLE".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("send to the wedging target should be accepted");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    let dropped_message_id = results[0].message_id.clone();

    // Wait past the 300ms prime timeout so the outcome resolves and the receipt
    // drops while alpha still has no worker.
    std::thread::sleep(Duration::from_millis(900));

    // Now prime alpha and deliver a sentinel; the dropped receipt must not appear.
    let sentinel = "SENTINEL-AFTER-DROP";
    dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-sentinel".to_string()),
            requester_session: "bravo".to_string(),
            message: sentinel.to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("sentinel send should be accepted");
    wait_for_pane_contains(
        &paths.tmux_socket,
        "alpha",
        sentinel,
        Duration::from_secs(5),
    );

    let snapshot = capture_pane(&paths.tmux_socket, "alpha", "-200");
    let flattened = snapshot.replace('\n', "");
    assert!(
        !flattened.contains("was not delivered"),
        "a dropped receipt must not resurface, snapshot={snapshot:?}"
    );
    assert!(
        !flattened.contains(&dropped_message_id),
        "the dropped receipt's message id must not appear, snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

/// A *delivered* terminal outcome produces NO receipt. Only non-delivered
/// outcomes are surfaced back to the sender; a success is recorded in `relay.log`
/// alone.
///
/// `alpha` (primed) sends to responsive `charlie`; the send is delivered. A later
/// sentinel delivered to `alpha` acts as a FIFO checkpoint: any receipt for the
/// delivered send would have been enqueued to `alpha`'s worker before the
/// sentinel, so observing the sentinel without a receipt proves none was sent.
#[test]
fn relay_send_async_emits_no_receipt_for_a_delivered_outcome() {
    if !tmux_available() {
        eprintln!("skipping delivered-no-receipt test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "charlie"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");
    spawn_session(&paths.tmux_socket, "charlie", "exec sleep 45");

    // Prime alpha's delivery worker (charlie -> alpha).
    dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-prime".to_string()),
            requester_session: "charlie".to_string(),
            message: "PRIME-ALPHA".to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("priming send should be accepted");
    wait_for_pane_contains(
        &paths.tmux_socket,
        "alpha",
        "PRIME-ALPHA",
        Duration::from_millis(3_000),
    );

    // alpha sends to responsive charlie; this delivers successfully.
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-delivered".to_string()),
            requester_session: "alpha".to_string(),
            message: "DELIVERED-PAYLOAD".to_string(),
            targets: vec!["charlie@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("send to charlie should be accepted");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    let delivered_message_id = results[0].message_id.clone();
    // Confirm the delivery landed, so its terminal outcome has resolved.
    wait_for_pane_contains(
        &paths.tmux_socket,
        "charlie",
        "DELIVERED-PAYLOAD",
        Duration::from_secs(5),
    );

    // Sentinel to alpha: a FIFO checkpoint after the delivered outcome resolved.
    let sentinel = "SENTINEL-AFTER-DELIVERED";
    dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-sentinel".to_string()),
            requester_session: "charlie".to_string(),
            message: sentinel.to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("sentinel send should be accepted");
    wait_for_pane_contains(
        &paths.tmux_socket,
        "alpha",
        sentinel,
        Duration::from_secs(5),
    );

    // No receipt for the delivered send may precede the sentinel in alpha's pane.
    // Flatten newlines so a wrapped receipt could not slip past the check.
    let snapshot = capture_pane(&paths.tmux_socket, "alpha", "-200");
    let flattened = snapshot.replace('\n', "");
    assert!(
        !flattened.contains("was not delivered"),
        "a delivered outcome must not produce a receipt, snapshot={snapshot:?}"
    );
    assert!(
        !flattened.contains(&delivered_message_id),
        "the delivered send's id must not appear in a receipt, snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
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
