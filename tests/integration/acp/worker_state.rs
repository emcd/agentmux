use std::{
    thread,
    time::{Duration, Instant},
};

use agentmux::relay::{ChatOutcome, ChatStatus, RelayResponse};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_worker_state_transitions_busy_then_available() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let config_root_for_thread = config_root.clone();
    let tmux_socket_for_thread = tmux_socket.clone();
    let handle = thread::spawn(move || {
        dispatch_send(
            &config_root_for_thread,
            &tmux_socket_for_thread,
            Some(2_000),
        )
    });

    let deadline = Instant::now() + Duration::from_millis(800);
    let mut observed_busy = false;
    while Instant::now() < deadline {
        if read_worker_state(temporary.path(), "bravo").as_deref() == Some("busy") {
            observed_busy = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        observed_busy,
        "expected worker_state=busy before completion"
    );

    let response = handle.join().expect("join send thread");
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(2)
        ),
        "worker_state did not converge to available"
    );
}

#[test]
fn acp_request_permission_marks_worker_busy_until_completion() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        request_permission_on_prompt: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let config_root_for_thread = config_root.clone();
    let tmux_socket_for_thread = tmux_socket.clone();
    let handle = thread::spawn(move || {
        dispatch_send(&config_root_for_thread, &tmux_socket_for_thread, Some(100))
    });

    let deadline = Instant::now() + Duration::from_millis(800);
    let mut observed_busy = false;
    while Instant::now() < deadline {
        if read_worker_state(temporary.path(), "bravo").as_deref() == Some("busy") {
            observed_busy = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        observed_busy,
        "expected worker_state=busy while ACP requested permission"
    );

    let response = handle.join().expect("join send thread");
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(2)
        ),
        "worker_state did not converge to available"
    );
}

#[test]
fn acp_worker_state_transitions_to_unavailable_on_prompt_failure() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_prompt: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "unavailable",
            Duration::from_secs(2)
        ),
        "worker_state did not converge to unavailable"
    );
}

fn wait_for_worker_state(
    root: &std::path::Path,
    target_session: &str,
    expected: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if read_worker_state(root, target_session).as_deref() == Some(expected) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn acp_async_queue_overflow_returns_runtime_queue_full() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let mut overflow_response = None::<RelayResponse>;
    for _ in 0..70 {
        let response = dispatch_send_with_mode_result(
            &config_root,
            &tmux_socket,
            Some(2_000),
            ChatDeliveryMode::Async,
        );
        match response {
            Ok(response) => {
                if let RelayResponse::Error { error } = &response
                    && error.code == "runtime_acp_queue_full"
                {
                    overflow_response = Some(response);
                    break;
                }
            }
            Err(error) => {
                if error.code == "runtime_acp_queue_full" {
                    overflow_response = Some(RelayResponse::Error { error });
                    break;
                }
            }
        }
    }

    let Some(RelayResponse::Error { error }) = overflow_response else {
        panic!("expected at least one runtime_acp_queue_full overflow response");
    };
    assert_eq!(error.code, "runtime_acp_queue_full");
    let details = error.details.expect("overflow details");
    assert_eq!(details["target_session"], "bravo");
    assert_eq!(details["max_pending"], 64);
}
