use std::time::Duration;

use agentmux::relay::{RelayResponse, SendOutcome};
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

    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    // dispatch_send returns once the worker picks up the queued prompt; with a
    // 1s prompt delay the worker is mid-turn and must report busy, then
    // converge back to available once the turn completes.
    assert!(
        wait_for_worker_state(temporary.path(), "bravo", "busy", Duration::from_secs(2)),
        "expected worker_state=busy while the ACP turn is in flight"
    );
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(3)
        ),
        "worker_state did not converge to available"
    );
}

#[test]
fn acp_request_permission_keeps_worker_busy_while_pending_decision() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        request_permission_on_prompt: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    assert!(
        wait_for_worker_state(temporary.path(), "bravo", "busy", Duration::from_secs(2)),
        "expected worker_state=busy while ACP requested permission"
    );
    assert!(
        !wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_millis(500)
        ),
        "worker_state unexpectedly converged to available without permission decision"
    );
    assert_eq!(
        read_worker_state(temporary.path(), "bravo").as_deref(),
        Some("busy")
    );
}

#[test]
fn acp_worker_state_stays_available_after_protocol_error() {
    // A JSON-RPC error response to session/prompt is a logical error from
    // a still-responsive agent. Under the fire-and-forget design, the
    // persistent worker stays alive (Available) for subsequent prompts.
    // Only transport-level failures (broken pipe write, reader EOF) mark
    // the worker Unavailable.
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_prompt: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "available",
            Duration::from_secs(2)
        ),
        "worker_state did not converge to available after protocol error"
    );
    assert!(
        !wait_for_worker_state(
            temporary.path(),
            "bravo",
            "unavailable",
            Duration::from_millis(200)
        ),
        "worker_state unexpectedly converged to unavailable on a JSON-RPC error response"
    );
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
        let response = dispatch_send_result(&config_root, &tmux_socket);
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
