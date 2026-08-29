//! Termination signals: what the relay must finish before it exits, and how
//! the members it was still holding resolve.

use std::{
    fs,
    time::{Duration, Instant},
};

use agentmux::relay::{
    RelayRequest, RelayResponse, RelayStreamSession, SendOutcome, request_relay,
};
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

use crate::support::relay_delivery::{
    spawn_relay_with_fake_tmux, spawn_relay_with_fake_tmux_and_env, wait_for_relay_ready,
    write_acp_hang_bundle_configuration, write_bundle_configuration, write_fake_tmux_script,
};

/// How long a relay host gets to exit after a termination signal.
///
/// Derived from the host's own shutdown watchdog grace
/// (`RELAY_SHUTDOWN_WATCHDOG_GRACE_MS`, 5s): the watchdog forces the process out
/// at that point, so a relay that is going to exit at all has exited by then.
/// A bound below it does not test anything the relay promises — the graceful
/// path is permitted to spend the whole grace (worker drain, the async-delivery
/// drain, and per-bundle tmux teardown all bound themselves against it), so a
/// tighter wait fails a relay that is shutting down exactly as specified.
///
/// Deliberately *above* the watchdog rather than at it. A forced exit is
/// `process::exit(0)`, which satisfies a status check while skipping cleanup, so
/// letting the process arrive here lets the assertions that follow — socket
/// removal, session pruning, the terminal delivery outcome — be what fails.
/// Those name which part of shutdown broke; a timeout reports only that time
/// passed.
const RELAY_SIGNAL_EXIT_BUDGET: Duration = Duration::from_secs(8);

/// Regression for issues/relay/49: a relay hosting an ACP coder with an
/// in-flight turn (the agent received `session/prompt` but has not yet returned
/// a `stopReason`) must shut down cleanly and promptly on SIGTERM. The delivery
/// thread's bounded, shutdown-gated prompt wait observes the turn; because the
/// framed write already succeeded, the member resolves `delivered` at the write
/// (not `dropped_on_shutdown`), no ACP child outlives the relay, and the process
/// exits far inside systemd's 90s `TimeoutStopSec` rather than being SIGKILLed.
///
/// Before the transport-abstraction + prime-timeout landings, the blocking ACP
/// prompt wait sat on a tokio blocking-pool thread that `Runtime::drop` waited
/// on, pinning the process until SIGKILL. The discriminators here are the
/// graceful-shutdown contract assertions -- socket removed, the in-flight turn's
/// `delivered` inscription (evidence earned at the framed write), and no
/// surviving ACP child -- not the bounded `child.wait()` alone: the host's
/// shutdown watchdog force-exits with `process::exit(0)` after a grace window,
/// so a stuck delivery wait could still produce a clean exit inside the bound
/// while skipping the graceful cleanup that removes the socket and resolves the
/// in-flight send.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_sigterm_reaps_in_flight_acp_turn_without_sigkill() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let stub_path = temporary.path().join("acp_hang_stub.sh");
    let acp_log = temporary.path().join("acp_requests.log");
    let acp_pids = temporary.path().join("acp_child_pids.txt");
    let config_root = write_acp_hang_bundle_configuration(
        temporary.path(),
        bundle_name,
        &["alpha", "bravo"],
        &stub_path,
        &acp_log,
        &acp_pids,
    );
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let tmux_log = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &tmux_log);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_ready(&relay_socket).await;

    // Dispatch an async send to the ACP target; the worker bootstraps the stub
    // child and dispatches session/prompt, which the stub never completes.
    let send_response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-acp-inflight".to_string()),
            requester_session: "alpha".to_string(),
            message: "status?".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
    )
    .expect("queue async ACP send");
    let RelayResponse::Send { results, .. } = send_response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Wait until the stub logs the in-flight session/prompt: only then is the
    // delivery thread parked in the bounded prompt-completion wait -- the exact
    // state issues/relay/49 was about.
    let prompt_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::read_to_string(&acp_log)
            .map(|log| log.contains("\"method\":\"session/prompt\""))
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < prompt_deadline,
            "ACP stub never received session/prompt; log={:?}",
            fs::read_to_string(&acp_log).unwrap_or_default()
        );
        sleep(Duration::from_millis(25)).await;
    }

    // SIGTERM (the signal systemd delivers) with the ACP turn still in flight.
    let pid = child.id().expect("relay pid");
    let pid = i32::try_from(pid).expect("relay pid fits i32");
    let kill_result = unsafe { libc::kill(pid, libc::SIGTERM) };
    assert_eq!(kill_result, 0, "failed to send SIGTERM");

    // Bounded clean exit: far under systemd's 90s TimeoutStopSec. This is a
    // liveness floor, not the precise regression detector -- the host's shutdown
    // watchdog would force process::exit(0) within its grace window even if a
    // delivery wait stalled graceful teardown. The graceful-shutdown contract
    // asserted below (socket removed, delivered inscription, no surviving child)
    // is what actually catches a reintroduced stuck ACP wait.
    let wait_result = timeout(Duration::from_secs(15), child.wait()).await;
    let status = match wait_result {
        Ok(result) => result.expect("wait relay"),
        Err(_) => {
            // SIGKILL is escalation only after the graceful SIGTERM window
            // (the generation fence's bounded reap) failed — not a bypass.
            child.start_kill().expect("kill relay after timeout");
            panic!("relay did not exit within 15s of SIGTERM with an in-flight ACP turn");
        }
    };
    assert!(
        status.success(),
        "relay should exit cleanly (no SIGKILL) after SIGTERM, status={status}"
    );
    assert!(
        !relay_socket.exists(),
        "relay socket should be removed during shutdown"
    );

    // The in-flight ACP turn resolved `delivered` at its framed write (the hang
    // stub logged the `session/prompt` request before going silent), so shutdown
    // does not downgrade it to `dropped_on_shutdown`.
    let inscriptions =
        fs::read_to_string(inscriptions_root.join("relay.log")).expect("read relay inscriptions");
    assert!(
        inscriptions.contains("\"event\":\"relay.send.async.completed\"")
            && inscriptions.contains("\"outcome\":\"delivered\""),
        "expected the in-flight ACP turn to resolve delivered at the framed write, \
         inscriptions={inscriptions:?}"
    );

    // No ACP stub child outlives the relay: each recorded child PID is gone
    // within a bounded window -- killed by the shutdown path, or reaped via
    // stdin EOF when the relay exits. A leaked/orphaned child would persist.
    let recorded_pids: Vec<i32> = fs::read_to_string(&acp_pids)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
        .collect();
    assert!(
        !recorded_pids.is_empty(),
        "the ACP stub should have recorded at least one child PID"
    );
    let reap_deadline = Instant::now() + Duration::from_secs(3);
    for child_pid in recorded_pids {
        loop {
            // kill(pid, 0) probes liveness without delivering a signal: it
            // returns -1 (ESRCH) once the process is gone.
            if unsafe { libc::kill(child_pid, 0) } == -1 {
                break;
            }
            assert!(
                Instant::now() < reap_deadline,
                "ACP stub child {child_pid} still alive after relay shutdown"
            );
            sleep(Duration::from_millis(25)).await;
        }
    }
}

/// Depends on a queued, readiness-blocked delivery surviving until shutdown,
/// which is now what relay-side admission produces: the member is held
/// `Pending` against an unready target instead of being parked inside a
/// transport-owned wait, and shutdown is what resolves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_sigint_prunes_owned_sessions_and_reaps_tmux_server() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_ready(&relay_socket).await;

    let send_response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-shutdown-drop".to_string()),
            requester_session: "alpha".to_string(),
            message: "queued async message".to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
    )
    .expect("queue async request");
    let RelayResponse::Send { results, .. } = send_response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    let pid = child.id().expect("relay pid");
    let pid = i32::try_from(pid).expect("relay pid fits i32");
    let kill_result = unsafe { libc::kill(pid, libc::SIGINT) };
    assert_eq!(kill_result, 0, "failed to send SIGINT");

    let wait_result = timeout(RELAY_SIGNAL_EXIT_BUDGET, child.wait()).await;
    let status = match wait_result {
        Ok(result) => result.expect("wait relay"),
        Err(_) => {
            // SIGKILL is escalation only after the watchdog's own forced
            // exit failed to arrive — not a bypass.
            child.start_kill().expect("kill relay after timeout");
            panic!(
                "relay did not exit within {RELAY_SIGNAL_EXIT_BUDGET:?} of SIGINT, \
                 which is past its own shutdown watchdog: the watchdog thread \
                 failed to force the exit"
            );
        }
    };
    assert!(
        status.success(),
        "relay should exit cleanly after SIGINT, status={status}"
    );
    assert!(
        !relay_socket.exists(),
        "relay socket should be removed during shutdown"
    );

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    assert!(
        log.contains("kill-session -t =alpha"),
        "shutdown should prune owned session, log={log:?}"
    );
    assert!(
        log.contains("kill-server"),
        "shutdown should reap tmux server when no sessions remain, log={log:?}"
    );

    let inscriptions =
        fs::read_to_string(inscriptions_root.join("relay.log")).expect("read relay inscriptions");
    assert!(
        inscriptions.contains("\"event\":\"relay.send.async.completed\"")
            && inscriptions.contains("\"outcome\":\"dropped_on_shutdown\""),
        "expected dropped_on_shutdown async terminal inscription, inscriptions={inscriptions:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_sigint_ignores_server_exited_unexpectedly_during_shutdown_cleanup() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux_and_env(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
        &[(
            "FAKE_TMUX_EMPTY_LIST_ERROR_MODE",
            "server_exited_unexpectedly",
        )],
    );
    wait_for_relay_ready(&relay_socket).await;

    let pid = child.id().expect("relay pid");
    let pid = i32::try_from(pid).expect("relay pid fits i32");
    let kill_result = unsafe { libc::kill(pid, libc::SIGINT) };
    assert_eq!(kill_result, 0, "failed to send SIGINT");

    let wait_result = timeout(RELAY_SIGNAL_EXIT_BUDGET, child.wait()).await;
    let status = match wait_result {
        Ok(result) => result.expect("wait relay"),
        Err(_) => {
            // SIGKILL is escalation only after the watchdog's own forced
            // exit failed to arrive — not a bypass.
            child.start_kill().expect("kill relay after timeout");
            panic!(
                "relay did not exit within {RELAY_SIGNAL_EXIT_BUDGET:?} of SIGINT, \
                 which is past its own shutdown watchdog: the watchdog thread \
                 failed to force the exit"
            );
        }
    };
    assert!(
        status.success(),
        "relay should exit cleanly after SIGINT, status={status}"
    );
    assert!(
        !relay_socket.exists(),
        "relay socket should be removed during shutdown"
    );

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    assert!(
        log.contains("kill-session -t =alpha"),
        "shutdown should still prune owned session, log={log:?}"
    );
    assert!(
        log.contains("kill-server"),
        "shutdown should still attempt tmux server cleanup, log={log:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_sigint_exits_with_active_stream_connection() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_ready(&relay_socket).await;

    let mut stream_session = RelayStreamSession::new(
        relay_socket.clone(),
        bundle_name.to_string(),
        "alpha".to_string(),
    );
    let stream_list_response = stream_session
        .request(&RelayRequest::List {
            requester_session: Some("alpha".to_string()),
        })
        .expect("list request on persistent stream");
    let RelayResponse::List { .. } = stream_list_response else {
        panic!("expected list response on persistent stream");
    };

    let pid = child.id().expect("relay pid");
    let pid = i32::try_from(pid).expect("relay pid fits i32");
    let kill_result = unsafe { libc::kill(pid, libc::SIGINT) };
    assert_eq!(kill_result, 0, "failed to send SIGINT");

    let wait_result = timeout(RELAY_SIGNAL_EXIT_BUDGET, child.wait()).await;
    let status = match wait_result {
        Ok(result) => result.expect("wait relay"),
        Err(_) => {
            // SIGKILL is escalation only after the watchdog's own forced
            // exit failed to arrive — not a bypass.
            child.start_kill().expect("kill relay after timeout");
            panic!(
                "relay did not exit within {RELAY_SIGNAL_EXIT_BUDGET:?} of SIGINT, \
                 which is past its own shutdown watchdog: the watchdog thread \
                 failed to force the exit"
            );
        }
    };
    assert!(
        status.success(),
        "relay should exit cleanly after SIGINT, status={status}"
    );
    assert!(
        !relay_socket.exists(),
        "relay socket should be removed during shutdown"
    );

    drop(stream_session);
}
