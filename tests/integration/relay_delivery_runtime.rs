use std::{
    fs,
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use agentmux::relay::{
    ListedSessionTransport, RelayRequest, RelayResponse, RelayStreamSession, SendOutcome,
    request_relay,
};
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

use crate::support::relay_delivery::{
    drain_child_stdout, spawn_relay_with_fake_tmux, spawn_relay_with_fake_tmux_and_env,
    wait_for_relay_ready, write_acp_hang_bundle_configuration, write_bundle_configuration,
    write_bundle_configuration_with_environment, write_bundle_with_pubsub_member,
    write_fake_tmux_script,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_startup_retries_transient_tmux_create_failures() {
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

    let started = Instant::now();
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_ready(&relay_socket).await;
    let elapsed = started.elapsed();

    let stdout = drain_child_stdout(&mut child).await;
    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    assert!(
        stdout.contains("\"host_mode\":\"autostart\""),
        "relay should report successful startup, stdout={stdout:?}"
    );
    let attempts = fs::read_to_string(&attempts_file)
        .expect("read attempts")
        .trim()
        .parse::<usize>()
        .expect("parse attempts");
    assert_eq!(attempts, 3, "relay should retry transient create failures");
    assert!(
        elapsed >= Duration::from_millis(50),
        "retry delays should be observable, elapsed={elapsed:?}"
    );
}

/// The merged bundle environment must reach the tmux session-creation call as
/// `new-session -e KEY=VALUE` flags (a plain `Command::env` on the tmux client
/// would not propagate into the pane's child). Boots an autostart bundle whose
/// bundle file declares a top-level `environment`, then asserts the fake tmux's
/// recorded argv for the owned session carries the `-e` flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_creates_tmux_session_with_environment_flags() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration_with_environment(
        temporary.path(),
        bundle_name,
        "alpha",
        &[("TMUX_ENV_PROBE", "on")],
    );
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

    // The autostart reconciler creates the owned session shortly after
    // readiness; poll the recorded argv log until the new-session call lands.
    let deadline = Instant::now() + Duration::from_secs(5);
    let new_session_line = loop {
        let log = fs::read_to_string(&log_file).unwrap_or_default();
        if let Some(line) = log
            .lines()
            .find(|line| line.contains("new-session") && line.contains("-s alpha"))
        {
            break line.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for tmux new-session, log={log:?}"
        );
        sleep(Duration::from_millis(50)).await;
    };
    assert!(
        new_session_line.contains("-e TMUX_ENV_PROBE=on"),
        "new-session must carry the merged environment as -e flags, line={new_session_line:?}"
    );

    let pid = child.id().expect("relay pid");
    let pid = i32::try_from(pid).expect("relay pid fits i32");
    let kill_result = unsafe { libc::kill(pid, libc::SIGINT) };
    assert_eq!(kill_result, 0, "failed to send SIGINT");
    let _ = timeout(Duration::from_secs(3), child.wait()).await;
}

/// Regression for issues/relay/49: a relay hosting an ACP coder with an
/// in-flight turn (the agent received `session/prompt` but has not yet returned
/// a `stopReason`) must shut down cleanly and promptly on SIGTERM. The delivery
/// thread's bounded, shutdown-gated prompt wait resolves the turn
/// `dropped_on_shutdown`, no ACP child outlives the relay, and the process exits
/// far inside systemd's 90s `TimeoutStopSec` rather than being SIGKILLed.
///
/// Before the transport-abstraction + prime-timeout landings, the blocking ACP
/// prompt wait sat on a tokio blocking-pool thread that `Runtime::drop` waited
/// on, pinning the process until SIGKILL. The discriminators here are the
/// graceful-shutdown contract assertions -- socket removed, the in-flight turn's
/// `dropped_on_shutdown` inscription, and no surviving ACP child -- not the
/// bounded `child.wait()` alone: the host's shutdown watchdog force-exits with
/// `process::exit(0)` after a grace window, so a stuck delivery wait could still
/// produce a clean exit inside the bound while skipping the graceful cleanup
/// that removes the socket and resolves the in-flight send.
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
            quiet_window_ms: Some(50),
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
    // asserted below (socket removed, dropped_on_shutdown, no surviving child) is
    // what actually catches a reintroduced stuck ACP wait.
    let wait_result = timeout(Duration::from_secs(15), child.wait()).await;
    let status = match wait_result {
        Ok(result) => result.expect("wait relay"),
        Err(_) => {
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

    // The in-flight ACP turn is resolved terminally by shutdown, not left hanging.
    let inscriptions =
        fs::read_to_string(inscriptions_root.join("relay.log")).expect("read relay inscriptions");
    assert!(
        inscriptions.contains("\"event\":\"relay.send.async.completed\"")
            && inscriptions.contains("\"outcome\":\"dropped_on_shutdown\""),
        "expected the in-flight ACP turn to resolve dropped_on_shutdown, \
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
            quiet_window_ms: None,
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

    let wait_result = timeout(Duration::from_secs(3), child.wait()).await;
    let status = match wait_result {
        Ok(result) => result.expect("wait relay"),
        Err(_) => {
            child.start_kill().expect("kill relay after timeout");
            panic!("timed out waiting for relay to exit after SIGINT");
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

/// A send to a configured `pubsub` member is refused synchronously at admission,
/// against a live relay: the request itself fails, nothing is queued, and no
/// terminal outcome is produced because nothing was accepted. It must also NOT
/// fall through to tmux delivery — regressing the construct-from-`session_type()`
/// model against the prior registry-routing default that misrouted non-UI targets
/// to tmux.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_send_to_configured_pubsub_member_is_refused_at_admission_and_skips_tmux() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_with_pubsub_member(temporary.path(), bundle_name, "alpha", "pub1");
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
            request_id: Some("req-pubsub".to_string()),
            requester_session: "alpha".to_string(),
            message: "to a pubsub member".to_string(),
            targets: vec!["pub1@party".to_string()],
            broadcast: false,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
    )
    .expect("relay answers the send request");
    let RelayResponse::Error { error } = send_response else {
        panic!("a pubsub target should be refused at admission, not answered with a send response");
    };
    assert_eq!(error.code, "runtime_session_type_not_implemented");

    // Wait for this request's own trace before asserting what is absent from it.
    // `relay.send.request` is emitted before authorization and admission, so its
    // presence means the log holds the refused request rather than nothing at all
    // — which is what would make the absence assertion below vacuous.
    let inscriptions_path = inscriptions_root.join("relay.log");
    let deadline = Instant::now() + Duration::from_secs(5);
    let inscriptions = loop {
        let current = fs::read_to_string(&inscriptions_path).unwrap_or_default();
        if current.contains("\"event\":\"relay.send.request\"") {
            break current;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for the send request inscription, inscriptions={current:?}");
        }
        sleep(Duration::from_millis(50)).await;
    };

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    // Nothing was accepted for pub1, so it has no queue entry and no terminal
    // outcome: no work is authorized merely to discover the stub.
    assert!(
        !inscriptions.contains("\"target_session\":\"pub1\""),
        "a refused pubsub target should produce no queue entry or outcome, inscriptions={inscriptions:?}"
    );
    let tmux_log = fs::read_to_string(&log_file).unwrap_or_default();
    assert!(
        !tmux_log.contains("pub1"),
        "pubsub target must not attempt tmux delivery, tmux_log={tmux_log:?}"
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

    let wait_result = timeout(Duration::from_secs(3), child.wait()).await;
    let status = match wait_result {
        Ok(result) => result.expect("wait relay"),
        Err(_) => {
            child.start_kill().expect("kill relay after timeout");
            panic!("timed out waiting for relay to exit after SIGINT");
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

    let wait_result = timeout(Duration::from_secs(3), child.wait()).await;
    let status = match wait_result {
        Ok(result) => result.expect("wait relay"),
        Err(_) => {
            child.start_kill().expect("kill relay after timeout");
            panic!("timed out waiting for relay to exit after SIGINT");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_accepts_new_connections_while_registered_stream_stays_open() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
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

    let relay_socket_for_second_request = relay_socket.clone();
    let second_list_response = timeout(
        Duration::from_millis(800),
        tokio::task::spawn_blocking(move || {
            request_relay(
                &relay_socket_for_second_request,
                "party",
                "bravo",
                &RelayRequest::List {
                    requester_session: Some("alpha".to_string()),
                },
            )
        }),
    )
    .await
    .expect("timed out waiting for second list response")
    .expect("join second list request task")
    .expect("second list request");
    let RelayResponse::List { .. } = second_list_response else {
        panic!("expected list response for second request");
    };

    drop(stream_session);
    child.start_kill().expect("kill relay");
    let _ = child.wait().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_rejects_connections_when_worker_queue_is_full() {
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
        &[("AGENTMUX_RELAY_MAX_CONNECTIONS", "2")],
    );
    wait_for_relay_ready(&relay_socket).await;

    let mut stream_session = RelayStreamSession::new(
        relay_socket.clone(),
        bundle_name.to_string(),
        "alpha".to_string(),
    );
    let first_response = stream_session
        .request(&RelayRequest::List {
            requester_session: Some("alpha".to_string()),
        })
        .expect("first stream list request");
    let RelayResponse::List { .. } = first_response else {
        panic!("expected list response from first stream");
    };

    let queued_stream = UnixStream::connect(&relay_socket).expect("connect queued stream");
    let rejected_stream = UnixStream::connect(&relay_socket).expect("connect rejected stream");
    let rejected_line = timeout(
        Duration::from_millis(800),
        tokio::task::spawn_blocking(move || {
            let mut rejected_reader = BufReader::new(rejected_stream);
            let mut line = String::new();
            rejected_reader
                .read_line(&mut line)
                .expect("read overload response");
            line
        }),
    )
    .await
    .expect("timed out waiting for overload response")
    .expect("join overload response task");
    let rejected_envelope: serde_json::Value =
        serde_json::from_str(rejected_line.trim_end()).expect("decode overload response");
    assert_eq!(
        rejected_envelope
            .get("frame")
            .and_then(serde_json::Value::as_str),
        Some("response")
    );
    let rejected_response: RelayResponse = serde_json::from_value(
        rejected_envelope
            .get("response")
            .cloned()
            .expect("overload response envelope missing 'response' field"),
    )
    .expect("decode overload response payload");
    let RelayResponse::Error { error } = rejected_response else {
        panic!("expected overload error response");
    };
    assert_eq!(error.code, "runtime_connection_limit_reached");
    assert_eq!(error.message, "relay connection limit reached");

    drop(queued_stream);
    drop(stream_session);
    child.start_kill().expect("kill relay");
    let _ = child.wait().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_reaps_pre_hello_idle_connections_and_recovers_worker_capacity() {
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
        &[
            ("AGENTMUX_RELAY_MAX_CONNECTIONS", "2"),
            ("AGENTMUX_RELAY_PRE_HELLO_IDLE_TIMEOUT_MS", "150"),
        ],
    );
    wait_for_relay_ready(&relay_socket).await;

    let idle_active = UnixStream::connect(&relay_socket).expect("connect idle active stream");
    let idle_queued = UnixStream::connect(&relay_socket).expect("connect idle queued stream");
    sleep(Duration::from_millis(500)).await;

    let relay_socket_for_retry = relay_socket.clone();
    let recovered_response = timeout(
        Duration::from_millis(800),
        tokio::task::spawn_blocking(move || {
            request_relay(
                &relay_socket_for_retry,
                "party",
                "alpha",
                &RelayRequest::List {
                    requester_session: Some("alpha".to_string()),
                },
            )
        }),
    )
    .await
    .expect("timed out waiting for recovered list response")
    .expect("join recovered list request task")
    .expect("recovered list request");
    let RelayResponse::List { .. } = recovered_response else {
        panic!("expected list response after stale connection reap");
    };

    drop(idle_active);
    drop(idle_queued);
    child.start_kill().expect("kill relay");
    let _ = child.wait().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_delivery_sends_submit_in_separate_tmux_command() {
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
        &[("FAKE_TMUX_CAPTURE_MODE", "stable")],
    );
    wait_for_relay_ready(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-submit-separate-enter".to_string()),
            requester_session: "alpha".to_string(),
            message: "A".repeat(6_000),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(50),
            on_behalf_of: None,
        },
    )
    .expect("send request should succeed");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Async delivery runs in a background worker; wait for the fake tmux log
    // to record the unbracketed carriage-return paste (the submit) before
    // reaping the relay so the full paste-buffer command sequence is
    // observable.
    let delivery_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if fs::read_to_string(&log_file)
            .map(|log| log.lines().any(is_submit_paste_line))
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < delivery_deadline,
            "async delivery did not complete within timeout"
        );
        sleep(Duration::from_millis(20)).await;
    }

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    let log_lines: Vec<&str> = log.lines().collect();
    // The body is delivered as a single bracketed paste (`-p`); the submit
    // is a separate unbracketed paste carrying a bare carriage return. Both
    // target the pane; only the body count is asserted as exactly-one here
    // (a chunked large payload would show more than one bracketed paste).
    let body_indexes: Vec<usize> = log_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_body_paste_line(line))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        body_indexes.len(),
        1,
        "expected exactly one bracketed body paste for large payload, log={log:?}"
    );
    let buffer_content = read_paste_buffer_content(&log_file, log_lines[body_indexes[0]]);
    assert!(
        buffer_content.contains("Message-Id:"),
        "expected pane envelope to include Message-Id header, content={buffer_content:?}"
    );
    assert!(
        buffer_content.contains("Date:"),
        "expected pane envelope to include Date header, content={buffer_content:?}"
    );
    assert!(
        buffer_content.contains("From:"),
        "expected pane envelope to include From header, content={buffer_content:?}"
    );
    assert!(
        buffer_content.contains("To:"),
        "expected pane envelope to include To header, content={buffer_content:?}"
    );
    assert!(
        buffer_content.starts_with("--agentmux-"),
        "expected paste buffer to begin with leading boundary fence, content={buffer_content:?}"
    );
    assert!(
        !buffer_content.contains("Envelope-Version:"),
        "pane envelope must omit Envelope-Version header, content={buffer_content:?}"
    );
    assert!(
        !buffer_content.contains("multipart/mixed; boundary="),
        "pane envelope must omit top-level multipart header, content={buffer_content:?}"
    );
    assert!(
        !buffer_content.contains("Content-Transfer-Encoding:"),
        "pane envelope must omit per-part transfer encoding header, content={buffer_content:?}"
    );
    let submit_index = log_lines
        .iter()
        .position(|line| is_submit_paste_line(line))
        .expect("expected separate unbracketed carriage-return paste (the submit)");
    assert!(
        body_indexes[0] < submit_index,
        "expected body paste before submit paste, log={log:?}"
    );
    assert_eq!(
        read_paste_buffer_content(&log_file, log_lines[submit_index]),
        "\r",
        "submit paste must carry a bare carriage return"
    );
    assert!(
        !log.contains("send-keys"),
        "submit must go through paste-buffer, not send-keys, log={log:?}"
    );

    let inscriptions =
        fs::read_to_string(inscriptions_root.join("relay.log")).expect("read relay inscriptions");
    // Isolate the envelope-metadata event itself: a whole-log substring scan
    // would match `bundle_name`/`namespace` carried by unrelated inscriptions
    // (e.g. relay.send.async.queued), so assert on this event's own `details`.
    let metadata_details = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| {
            event.get("event").and_then(serde_json::Value::as_str)
                == Some("relay.send.envelope.metadata")
        })
        .and_then(|event| event.get("details").cloned())
        .expect("expected a relay.send.envelope.metadata inscription with details");
    let details = metadata_details
        .as_object()
        .expect("envelope metadata details is an object");
    for field in [
        "schema_version",
        "message_id",
        "namespace",
        "sender_session",
        "target_sessions",
        "created_at",
    ] {
        assert!(
            details.contains_key(field),
            "envelope metadata must include {field}, details={details:?}"
        );
    }
    assert!(
        !details.contains_key("bundle_name"),
        "envelope metadata must use namespace, not the retired bundle_name field, \
         details={details:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delivery_progress_inscription_carries_group_and_namespace() {
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
        &[
            ("FAKE_TMUX_CAPTURE_MODE", "stable"),
            ("AGENTMUX_RELAY_DELIVERY_DIAGNOSTICS", "true"),
        ],
    );
    wait_for_relay_ready(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        bundle_name,
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-delivery-progress".to_string()),
            requester_session: "alpha".to_string(),
            message: "diagnostic correlation".to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(10),
            on_behalf_of: None,
        },
    )
    .expect("send request should succeed");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    let inscriptions_path = inscriptions_root.join("relay.log");
    let deadline = Instant::now() + Duration::from_secs(5);
    let diagnostic = loop {
        let current = fs::read_to_string(&inscriptions_path).unwrap_or_default();
        if let Some(value) = current
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|value| value["event"] == "relay.delivery_ready")
        {
            break value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for delivery progress, inscriptions={current:?}"
        );
        sleep(Duration::from_millis(20)).await;
    };

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    let details = &diagnostic["details"];
    assert_eq!(details["namespace"], bundle_name);
    assert_eq!(details["target_session"], "alpha");
    assert_eq!(details["message_ids_total"], 1);
    let message_ids = details["message_ids"]
        .as_array()
        .expect("delivery progress message_ids array");
    assert_eq!(message_ids.len(), 1);
    assert!(
        message_ids[0]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "delivery progress must carry the queued message id: {diagnostic}"
    );
}

/// A pane reporting `#{pane_in_mode} = 1` (tmux copy-mode, as a
/// mouse-wheel scroll leaves it) must NOT block delivery: the classifier
/// ignores copy-mode, so the message is both pasted and submitted. This
/// asserts the command shape our code emits under copy-mode — the fake
/// tmux cannot model paste-through-copy-mode semantics, only the command
/// sequence; the real-tmux behavior is covered separately in
/// relay_delivery_async.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_async_delivery_injects_even_while_pane_in_mode() {
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
        &[
            ("FAKE_TMUX_CAPTURE_MODE", "stable"),
            ("FAKE_TMUX_PANE_IN_MODE", "1"),
        ],
    );
    wait_for_relay_ready(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-interaction-mode".to_string()),
            requester_session: "alpha".to_string(),
            message: "interaction marker".to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(50),
            on_behalf_of: None,
        },
    )
    .expect("send request should complete");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Delivery must proceed to the submit despite pane_in_mode=1. Wait for
    // the unbracketed carriage-return paste; absent the gate removal this
    // would never appear.
    let delivery_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if fs::read_to_string(&log_file)
            .map(|log| log.lines().any(is_submit_paste_line))
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < delivery_deadline,
            "delivery did not reach submit while pane_in_mode active"
        );
        sleep(Duration::from_millis(20)).await;
    }

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    assert!(
        log.lines().any(is_body_paste_line),
        "body must be pasted even while pane_in_mode active, log={log:?}"
    );
    assert!(
        !log.contains("send-keys"),
        "delivery must go through paste-buffer, not send-keys, log={log:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_raww_tmux_default_queues_and_appends_enter() {
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

    let response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Raww {
            request_id: Some("req-raww-default-enter".to_string()),
            requester_session: "alpha".to_string(),
            target_session: "alpha@party".to_string(),
            text: "hello from raww".to_string(),
            no_enter: false,
            on_behalf_of: None,
        },
    )
    .expect("raww request should succeed");

    let RelayResponse::Raww {
        status,
        target_session,
        transport,
        request_id,
        message_id,
        ..
    } = response
    else {
        panic!("expected raww response");
    };
    assert_eq!(status, "queued");
    assert_eq!(target_session, "alpha@party");
    assert_eq!(transport, ListedSessionTransport::Tmux);
    assert_eq!(request_id.as_deref(), Some("req-raww-default-enter"));
    assert!(message_id.is_some(), "message_id should be present");

    // Raww delivery now runs in a background worker; wait for the unbracketed
    // carriage-return paste (the submit) before reaping the relay so the full
    // paste-buffer sequence is observable in the log.
    let delivery_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_to_string(&log_file)
            .map(|log| log.lines().any(is_submit_paste_line))
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < delivery_deadline,
            "async raww delivery did not complete within timeout"
        );
        sleep(Duration::from_millis(20)).await;
    }

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    let body_line = log
        .lines()
        .find(|line| is_body_paste_line(line))
        .expect("expected bracketed body paste in fake tmux log");
    let buffer_content = read_paste_buffer_content(&log_file, body_line);
    assert_eq!(
        buffer_content, "hello from raww",
        "expected paste buffer to carry literal raww text, content={buffer_content:?}"
    );
    let submit_line = log
        .lines()
        .find(|line| is_submit_paste_line(line))
        .expect("expected unbracketed carriage-return paste for default raww behavior");
    assert_eq!(
        read_paste_buffer_content(&log_file, submit_line),
        "\r",
        "submit paste must carry a bare carriage return"
    );
    assert!(
        !log.contains("send-keys"),
        "submit must go through paste-buffer, not send-keys, log={log:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_raww_tmux_no_enter_omits_enter_command() {
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

    let response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Raww {
            request_id: Some("req-raww-no-enter".to_string()),
            requester_session: "alpha".to_string(),
            target_session: "alpha@party".to_string(),
            text: "hello without enter".to_string(),
            no_enter: true,
            on_behalf_of: None,
        },
    )
    .expect("raww request should succeed");

    let RelayResponse::Raww {
        status,
        target_session,
        transport,
        request_id,
        ..
    } = response
    else {
        panic!("expected raww response");
    };
    assert_eq!(status, "queued");
    assert_eq!(target_session, "alpha@party");
    assert_eq!(transport, ListedSessionTransport::Tmux);
    assert_eq!(request_id.as_deref(), Some("req-raww-no-enter"));

    // Async delivery: no_enter sends no submit, so wait for the body
    // paste-buffer command itself before reaping the relay.
    let delivery_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_to_string(&log_file)
            .map(|log| log.lines().any(is_body_paste_line))
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < delivery_deadline,
            "async raww delivery did not complete within timeout"
        );
        sleep(Duration::from_millis(20)).await;
    }

    // Give any (erroneous) submit paste a chance to appear before asserting
    // its absence.
    sleep(Duration::from_millis(200)).await;

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    let body_line = log
        .lines()
        .find(|line| is_body_paste_line(line))
        .expect("expected bracketed body paste in fake tmux log");
    let buffer_content = read_paste_buffer_content(&log_file, body_line);
    assert_eq!(
        buffer_content, "hello without enter",
        "expected paste buffer to carry literal raww text, content={buffer_content:?}"
    );
    assert!(
        !log.lines().any(is_submit_paste_line),
        "did not expect a submit carriage-return paste when no_enter=true, log={log:?}"
    );
    assert!(
        !log.contains("send-keys"),
        "no_enter delivery must not emit send-keys, log={log:?}"
    );
}

/// A bracketed (`-p`) paste of the message body into the target pane.
fn is_body_paste_line(line: &str) -> bool {
    line.contains(" paste-buffer ") && line.contains("-t %1") && line.contains(" -p ")
}

/// The unbracketed paste carrying the submit carriage return. Distinguished
/// from the body paste by the absence of the `-p` (bracketed) flag.
fn is_submit_paste_line(line: &str) -> bool {
    line.contains(" paste-buffer ") && line.contains("-t %1") && !line.contains(" -p ")
}

fn read_paste_buffer_content(log_file: &Path, paste_line: &str) -> String {
    let mut tokens = paste_line.split_whitespace();
    let buffer_name = tokens
        .by_ref()
        .skip_while(|token| *token != "-b")
        .nth(1)
        .expect("paste-buffer command should include -b NAME");
    let buffer_path = PathBuf::from(format!("{}.buffer.{buffer_name}", log_file.display()));
    fs::read_to_string(&buffer_path)
        .unwrap_or_else(|error| panic!("read paste buffer file {}: {error}", buffer_path.display()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_async_delivery_envelope_addresses_carry_canonical_ids_across_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle_configuration(temporary.path(), "party", &["alpha", "bravo"]);
    // A second namespace plus a send scope that reaches it: the fixture's
    // policies file caps send at home.
    fs::write(
        config_root.base_layer().join("bundles").join("qa.toml"),
        r#"format-version = 1
autostart = true

[[sessions]]
id = "zulu"
name = "zulu"
directory = "/tmp"
coder = "default"
"#,
    )
    .expect("write qa bundle config");
    fs::write(
        config_root.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "all"
"#,
    )
    .expect("widen send scope for cross-bundle delivery");

    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux_and_env(
        "party",
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
        &[("FAKE_TMUX_CAPTURE_MODE", "stable")],
    );
    wait_for_relay_ready(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-canonical-addresses".to_string()),
            requester_session: "alpha".to_string(),
            message: "cross-bundle co-recipient visibility".to_string(),
            targets: vec!["bravo@party".to_string(), "zulu@qa".to_string()],
            broadcast: false,
            quiet_window_ms: Some(50),
            on_behalf_of: None,
        },
    )
    .expect("cross-bundle send request should succeed");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|result| result.outcome == SendOutcome::Queued),
        "expected both targets queued, results={results:?}"
    );

    // Async delivery pastes one buffer per target; wait until both envelopes
    // land, then identify each by its To header.
    let delivery_deadline = Instant::now() + Duration::from_secs(5);
    let (bravo_envelope, zulu_envelope) = loop {
        let envelopes = read_all_paste_buffers(temporary.path());
        let bravo = envelopes
            .iter()
            .find(|content| content.contains("To: bravo <session:bravo@party>"))
            .cloned();
        let zulu = envelopes
            .iter()
            .find(|content| content.contains("To: zulu <session:zulu@qa>"))
            .cloned();
        if let (Some(bravo), Some(zulu)) = (bravo, zulu) {
            break (bravo, zulu);
        }
        assert!(
            Instant::now() < delivery_deadline,
            "async deliveries did not complete, envelopes={envelopes:?}"
        );
        sleep(Duration::from_millis(20)).await;
    };

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    assert!(
        bravo_envelope.contains("From: alpha <session:alpha@party>"),
        "expected canonical sender address, envelope={bravo_envelope:?}"
    );
    // The cross-bundle co-recipient is absent from the delivery bundle's
    // configuration, so its Cc entry carries the canonical id alone.
    assert!(
        bravo_envelope.contains("Cc: zulu@qa <session:zulu@qa>"),
        "expected cross-bundle co-recipient in Cc, envelope={bravo_envelope:?}"
    );
    assert!(
        zulu_envelope.contains("From: alpha <session:alpha@party>"),
        "expected canonical sender address, envelope={zulu_envelope:?}"
    );
    assert!(
        zulu_envelope.contains("Cc: bravo@party <session:bravo@party>"),
        "expected cross-bundle co-recipient in Cc, envelope={zulu_envelope:?}"
    );
}

fn read_all_paste_buffers(directory: &Path) -> Vec<String> {
    let mut contents = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return contents;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        if file_name
            .to_string_lossy()
            .starts_with("fake-tmux.log.buffer.")
            && let Ok(content) = fs::read_to_string(entry.path())
        {
            contents.push(content);
        }
    }
    contents
}
