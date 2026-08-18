use std::time::Duration;

use agentmux::{
    configuration::ConfigurationRoots,
    relay::{
        ListedSessionTransport, RelayRequest, RelayResponse, SendOutcome, StartupFailureRecord,
        append_startup_failure, handle_request, load_startup_failures,
    },
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

use crate::support::relay_delivery::{
    TmuxServerGuard, capture_pane, spawn_session, tmux_available, tmux_command,
    wait_for_pane_contains, write_bundle_configuration,
};

fn dispatch_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    runtime_directory: &std::path::Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(request, configuration_roots, bundle_name, runtime_directory)
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

/// A delivered send clears the target's startup-failure records.
///
/// Two observations clear a record — a successful startup and a successful
/// delivery — and only the startup one had coverage. The distinction matters
/// because the two reach the same helper from opposite ends of the runtime: the
/// startup path calls it while bringing a session up, this one calls it from the
/// delivery worker after a terminal outcome resolves, and a refactor can remove
/// the second without any startup test noticing.
///
/// The unrelated `ghost` record is the control. Clearing is keyed per session,
/// so a test that only watched the target's records would pass equally well
/// against an implementation that wiped the whole bundle's history on delivery.
#[test]
fn relay_send_async_clears_the_targets_startup_failure_records_on_delivery() {
    if !tmux_available() {
        eprintln!("skipping delivery-clears-startup-failures test because tmux is unavailable");
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

    for (session_id, reason) in [
        ("charlie", "a failure charlie is about to disprove"),
        ("ghost", "an unrelated failure that must survive"),
    ] {
        append_startup_failure(
            &paths.runtime_directory,
            StartupFailureRecord {
                session_id: session_id.to_string(),
                transport: ListedSessionTransport::Tmux,
                code: "runtime_startup_failed".to_string(),
                reason: reason.to_string(),
                timestamp: "2026-05-01T00:00:00Z".to_string(),
                sequence: 0,
                details: None,
            },
        )
        .expect("append startup failure record");
    }

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-clears-history".to_string()),
            requester_session: "alpha".to_string(),
            message: "CLEARS-HISTORY-PAYLOAD".to_string(),
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
    assert_eq!(results[0].outcome, SendOutcome::Queued);
    wait_for_pane_contains(
        &paths.tmux_socket,
        "charlie",
        "CLEARS-HISTORY-PAYLOAD",
        Duration::from_secs(5),
    );

    // The pane content proves the write landed, which happens before the
    // terminal outcome resolves and therefore before the clearing runs. Polling
    // rather than asserting immediately is the difference between testing the
    // behaviour and testing the scheduler.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let remaining = loop {
        let records = load_startup_failures(&paths.runtime_directory).expect("load history");
        let remaining: Vec<String> = records
            .iter()
            .filter(|record| record.session_id == "charlie")
            .map(|record| record.reason.clone())
            .collect();
        if remaining.is_empty() || std::time::Instant::now() >= deadline {
            break records;
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let charlie_records: Vec<&str> = remaining
        .iter()
        .filter(|record| record.session_id == "charlie")
        .map(|record| record.reason.as_str())
        .collect();
    assert!(
        charlie_records.is_empty(),
        "a delivered send must clear the target's records, still holding {charlie_records:?}"
    );
    let ghost_records: Vec<&str> = remaining
        .iter()
        .filter(|record| record.session_id == "ghost")
        .map(|record| record.reason.as_str())
        .collect();
    assert_eq!(
        ghost_records,
        ["an unrelated failure that must survive"],
        "clearing is per session; an untouched session keeps its record"
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

/// A Tmux delivery is written while the target is still producing output;
/// delivery does not wait for a quiet pane or a prompt-readiness timeout.
#[test]
fn relay_send_async_writes_while_target_is_active() {
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
        "i=0; while [ \"$i\" -lt 300 ]; do printf '\\rWORK-%03d' \"$i\"; i=$((i+1)); sleep 0.02; done; printf '\\nIDLE\\n'; exec sleep 45",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "WORK-",
        Duration::from_millis(1_200),
    );

    let marker = "ASYNC-DIRECT-WRITE-MARKER";
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
        Duration::from_millis(2_000),
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

/// Mail and raw are one per-target FIFO, not two.
///
/// The existing FIFO test covers repeated mail. This one interleaves the two
/// request kinds against a single target, because the property that could break
/// is not ordering within a kind — each kind reaches the transport through its
/// own request handler, and nothing in either handler consults the other's
/// queue. What holds them in one order is that both call the same
/// `enqueue_async_delivery` for the same worker key, and that
/// `try_existing_worker` performs `sender.send` while holding the registry
/// lock. That lock is the linearization point: two concurrent senders to one
/// target serialize on it, and because the channel is unbounded the send cannot
/// block, so channel order is lock-acquisition order.
///
/// The guarantee is therefore **worker-enqueue linearization, not request or
/// admission order** — admission is reserved earlier, in each handler, so a
/// request may reserve quota first and still lose the race to enqueue. This
/// test drives the requests sequentially, which is what lets it assert an
/// expected order at all; a concurrent variant could only assert that some
/// single order was observed, which is a weaker property than the one at issue.
///
/// Ordering is read off the pane rather than off inscriptions so the assertion
/// covers the whole path, including the transport's internal channel, which is
/// what preserves a raw barrier's position relative to the envelopes around it.
#[test]
fn relay_interleaves_mail_and_raw_in_one_per_target_order() {
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
    spawn_session(&paths.tmux_socket, "bravo", "exec cat");

    // Mail, raw, mail — so a raw entry is bracketed by envelopes on both sides.
    // Ordering that only ever placed raw first or last could be produced by the
    // two kinds occupying separate queues drained in a fixed sequence, which is
    // precisely the arrangement this asserts against.
    let markers = ["ORDER-MAIL-ONE", "ORDER-RAW-TWO", "ORDER-MAIL-THREE"];

    let send_mail = |marker: &str, request_id: &str| {
        let response = dispatch_request(
            RelayRequest::Send {
                request_id: Some(request_id.to_string()),
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
    };

    send_mail(markers[0], "req-order-1");

    let raw = dispatch_request(
        RelayRequest::Raww {
            request_id: Some("req-order-2".to_string()),
            requester_session: "alpha".to_string(),
            target_session: "bravo@party".to_string(),
            text: markers[1].to_string(),
            no_enter: false,
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("raww request should be accepted");
    let RelayResponse::Raww { status, .. } = raw else {
        panic!("expected raww response");
    };
    assert_eq!(status, "queued");

    send_mail(markers[2], "req-order-3");

    for marker in markers {
        wait_for_pane_contains(
            &paths.tmux_socket,
            "bravo",
            marker,
            Duration::from_millis(4_000),
        );
    }

    let snapshot = capture_pane(&paths.tmux_socket, "bravo", "-200");
    let positions: Vec<usize> = markers
        .iter()
        .map(|marker| {
            snapshot
                .find(marker)
                .unwrap_or_else(|| panic!("marker {marker} should exist in pane"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "mail and raw must land in one per-target order; positions={positions:?}, \
         snapshot={snapshot:?}"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

/// A *non-delivered* outcome must produce a receipt that reaches the sender.
///
/// The positive control the suppression test above has always lacked. Asserting
/// that a delivered outcome produces no receipt says nothing on its own: a relay
/// that emitted no receipts at all, ever, would satisfy it. Only a fixture where
/// a receipt is required to arrive can tell suppression apart from a receipt path
/// that does not work.
///
/// Every condition the receipt path needs is arranged rather than assumed. The
/// sender has a live delivery worker, because a receipt routes only to an
/// existing one and is dropped rather than spawning it. The sender's pane is a
/// real tmux pane that can be read back. The failing target is unreachable from
/// its first observation — no session backs it — so its member resolves
/// `not_submitted` past the dwell, which is the outcome class that owes a
/// receipt.
#[test]
fn relay_send_async_delivers_a_receipt_for_a_non_delivered_outcome() {
    if !tmux_available() {
        eprintln!("skipping receipt-delivery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    agentmux::relay::configure_delivery(agentmux::relay::DeliveryConfiguration {
        unreachable_dwell_ms: 400,
        ..agentmux::relay::DeliveryConfiguration::default()
    });

    // Only alpha gets a session. bravo is configured but has no pane, so it is
    // unreachable from the first observation.
    spawn_session(&paths.tmux_socket, "alpha", "exec sleep 45");

    // Prime alpha's delivery worker, which the receipt needs to already exist.
    dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-prime-alpha".to_string()),
            requester_session: "bravo".to_string(),
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
        Duration::from_secs(5),
    );

    // alpha sends to the unreachable target; this is the send that owes a receipt.
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-undeliverable".to_string()),
            requester_session: "alpha".to_string(),
            message: "UNDELIVERABLE-PAYLOAD".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(70),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("send to the unreachable target should be accepted");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    let undelivered_message_id = results[0].message_id.clone();

    // The outcome resolves non-delivered, which is the precondition for a receipt.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let log = std::fs::read_to_string(&inscriptions).unwrap_or_default();
        if log.contains(undelivered_message_id.as_str())
            && log.contains("\"outcome\":\"not_submitted\"")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the unreachable target's member never resolved: {log}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // The receipt must reach alpha's pane.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = capture_pane(&paths.tmux_socket, "alpha", "-200").replace('\n', "");
        if snapshot.contains("was not delivered") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the sender was never told its message was not delivered; \
             alpha pane={snapshot:?} inscriptions={:?}",
            std::fs::read_to_string(&inscriptions).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}
