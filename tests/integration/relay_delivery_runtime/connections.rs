//! Connection handling: admitting new connections alongside a registered
//! stream, refusing them once the worker queue is full, and recovering that
//! capacity by reaping idle pre-hello connections.

use std::{
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    time::Duration,
};

use agentmux::relay::{RelayRequest, RelayResponse, RelayStreamSession, request_relay};
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

use crate::support::relay_delivery::{
    spawn_relay_with_fake_tmux, spawn_relay_with_fake_tmux_and_env, wait_for_relay_ready,
    write_bundle_configuration, write_fake_tmux_script,
};

use super::*;

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
    shutdown_relay_gracefully(&mut child).await;
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
    shutdown_relay_gracefully(&mut child).await;
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
    shutdown_relay_gracefully(&mut child).await;
}
