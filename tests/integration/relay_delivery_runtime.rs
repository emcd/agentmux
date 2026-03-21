use std::{
    fs,
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use agentmux::relay::{
    ChatDeliveryMode, ChatOutcome, ChatStatus, RelayRequest, RelayResponse, RelayStreamClientClass,
    RelayStreamSession, request_relay,
};
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

use crate::support::relay_delivery::{
    drain_child_stdout, spawn_relay_with_fake_tmux, spawn_relay_with_fake_tmux_and_env,
    wait_for_relay_socket, write_bundle_configuration, write_fake_tmux_script,
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

    let relay_socket = state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock");

    let started = Instant::now();
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_socket(&relay_socket).await;
    let elapsed = started.elapsed();

    let stdout = drain_child_stdout(&mut child).await;
    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    assert!(
        stdout.contains("agentmux host relay listening"),
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

    let relay_socket = state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_socket(&relay_socket).await;

    let chat_response = request_relay(
        &relay_socket,
        &RelayRequest::Chat {
            request_id: Some("req-shutdown-drop".to_string()),
            sender_session: "alpha".to_string(),
            message: "queued async message".to_string(),
            targets: vec!["alpha".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: None,
            quiescence_timeout_ms: None,
        },
    )
    .expect("queue async request");
    let RelayResponse::Chat {
        status,
        results,
        delivery_mode,
        ..
    } = chat_response
    else {
        panic!("expected chat response");
    };
    assert_eq!(delivery_mode, ChatDeliveryMode::Async);
    assert_eq!(status, ChatStatus::Accepted);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Queued);

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

    let inscriptions = fs::read_to_string(
        inscriptions_root
            .join("bundles")
            .join(bundle_name)
            .join("relay.log"),
    )
    .expect("read relay inscriptions");
    assert!(
        inscriptions.contains("\"event\":\"relay.chat.async.completed\"")
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

    let relay_socket = state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock");
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
    wait_for_relay_socket(&relay_socket).await;

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
async fn relay_accepts_new_connections_while_registered_stream_stays_open() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_socket(&relay_socket).await;

    let mut stream_session = RelayStreamSession::new(
        relay_socket.clone(),
        bundle_name.to_string(),
        "alpha".to_string(),
        RelayStreamClientClass::Agent,
    );
    let stream_list_response = stream_session
        .request(&RelayRequest::List {
            sender_session: Some("alpha".to_string()),
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
                &RelayRequest::List {
                    sender_session: Some("alpha".to_string()),
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

    let relay_socket = state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux_and_env(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
        &[
            ("AGENTMUX_RELAY_CONNECTION_WORKERS", "1"),
            ("AGENTMUX_RELAY_CONNECTION_QUEUE_CAPACITY", "1"),
        ],
    );
    wait_for_relay_socket(&relay_socket).await;

    let mut stream_session = RelayStreamSession::new(
        relay_socket.clone(),
        bundle_name.to_string(),
        "alpha".to_string(),
        RelayStreamClientClass::Agent,
    );
    let first_response = stream_session
        .request(&RelayRequest::List {
            sender_session: Some("alpha".to_string()),
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
    let rejected_response: RelayResponse =
        serde_json::from_str(rejected_line.trim_end()).expect("decode overload response");
    let RelayResponse::Error { error } = rejected_response else {
        panic!("expected overload error response");
    };
    assert_eq!(error.code, "runtime_connection_queue_full");
    assert_eq!(error.message, "relay connection worker pool queue is full");

    drop(queued_stream);
    drop(stream_session);
    child.start_kill().expect("kill relay");
    let _ = child.wait().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_sync_delivery_sends_submit_in_separate_tmux_command() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux_and_env(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
        &[("FAKE_TMUX_CAPTURE_MODE", "stable")],
    );
    wait_for_relay_socket(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        &RelayRequest::Chat {
            request_id: Some("req-submit-separate-enter".to_string()),
            sender_session: "alpha".to_string(),
            message: "A".repeat(6_000),
            targets: vec!["alpha".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Sync,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms: Some(2_000),
        },
    )
    .expect("chat request should succeed");
    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Delivered);

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    assert!(
        log.contains("Message-Id:"),
        "expected pane envelope to include Message-Id header, log={log:?}"
    );
    assert!(
        log.contains("Date:"),
        "expected pane envelope to include Date header, log={log:?}"
    );
    assert!(
        log.contains("From:"),
        "expected pane envelope to include From header, log={log:?}"
    );
    assert!(
        log.contains("To:"),
        "expected pane envelope to include To header, log={log:?}"
    );
    assert!(
        log.contains("--agentmux-"),
        "expected pane envelope boundary marker, log={log:?}"
    );
    assert!(
        !log.contains("Envelope-Version:"),
        "pane envelope must omit Envelope-Version header, log={log:?}"
    );
    assert!(
        !log.contains("multipart/mixed; boundary="),
        "pane envelope must omit top-level multipart header, log={log:?}"
    );
    assert!(
        !log.contains("Content-Transfer-Encoding:"),
        "pane envelope must omit per-part transfer encoding header, log={log:?}"
    );
    let send_keys_lines = log
        .lines()
        .filter(|line| line.contains(" send-keys "))
        .collect::<Vec<_>>();
    let prompt_indexes = send_keys_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("send-keys -l -t %1 --"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert!(
        !prompt_indexes.is_empty(),
        "expected at least one prompt send-keys command, log={log:?}"
    );
    assert!(
        prompt_indexes.len() > 1,
        "expected chunked prompt send-keys commands for large payload, log={log:?}"
    );
    let first_prompt_index = *prompt_indexes
        .first()
        .expect("expected at least one prompt command");
    assert!(
        send_keys_lines[first_prompt_index].contains("send-keys -l -t %1 -- --agentmux-"),
        "expected first prompt chunk to begin with leading boundary fence, log={log:?}"
    );
    let enter_index = send_keys_lines
        .iter()
        .position(|line| line.ends_with("send-keys -t %1 Enter"))
        .expect("expected separate Enter send-keys command");
    let last_prompt_index = *prompt_indexes
        .last()
        .expect("expected at least one prompt command");
    assert!(
        last_prompt_index < enter_index,
        "expected prompt command before Enter command, log={log:?}"
    );

    let inscriptions = fs::read_to_string(
        inscriptions_root
            .join("bundles")
            .join(bundle_name)
            .join("relay.log"),
    )
    .expect("read relay inscriptions");
    assert!(
        inscriptions.contains("\"event\":\"relay.chat.envelope.metadata\"")
            && inscriptions.contains("\"schema_version\"")
            && inscriptions.contains("\"message_id\"")
            && inscriptions.contains("\"bundle_name\"")
            && inscriptions.contains("\"sender_session\"")
            && inscriptions.contains("\"target_sessions\"")
            && inscriptions.contains("\"created_at\""),
        "expected out-of-band envelope metadata inscription, inscriptions={inscriptions:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_async_delivery_does_not_inject_while_pane_in_mode() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock");
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
    wait_for_relay_socket(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        &RelayRequest::Chat {
            request_id: Some("req-interaction-mode".to_string()),
            sender_session: "alpha".to_string(),
            message: "interaction marker".to_string(),
            targets: vec!["alpha".to_string()],
            broadcast: false,
            delivery_mode: ChatDeliveryMode::Async,
            quiet_window_ms: Some(50),
            quiescence_timeout_ms: Some(250),
        },
    )
    .expect("chat request should complete");
    let RelayResponse::Chat {
        status, results, ..
    } = response
    else {
        panic!("expected chat response");
    };
    assert_eq!(status, ChatStatus::Accepted);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, ChatOutcome::Queued);

    sleep(Duration::from_millis(500)).await;

    child.start_kill().expect("kill relay");
    let _ = child.wait().await;

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    assert!(
        !log.contains("send-keys"),
        "no send-keys should be injected while pane_in_mode stays active, log={log:?}"
    );
}
