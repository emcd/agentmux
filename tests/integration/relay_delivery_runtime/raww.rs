//! Raww delivery: literal text into the pane, and whether a submit follows.

use std::{
    fs,
    time::{Duration, Instant},
};

use agentmux::relay::{ListedSessionTransport, RelayRequest, RelayResponse, request_relay};
use tempfile::TempDir;
use tokio::time::sleep;

use crate::support::relay_delivery::{
    spawn_relay_with_fake_tmux, wait_for_relay_ready, write_bundle_configuration,
    write_fake_tmux_script,
};

use super::*;

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

    shutdown_relay_gracefully(&mut child).await;

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

    shutdown_relay_gracefully(&mut child).await;

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
