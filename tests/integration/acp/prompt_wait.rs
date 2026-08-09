//! Bounded single-flight prompt-completion wait.
//!
//! `AcpStdioClient::wait_for_prompt_complete` must be resumable and bounded so
//! the per-target worker can interleave a shutdown check between polls instead
//! of parking forever on an agent whose turn never completes (which would pin
//! the runtime's blocking pool and hang relay shutdown until SIGKILL). These
//! tests exercise the public client against the shared ACP stub agent.

use std::time::{Duration, Instant};

use agentmux::acp::{AcpStdioClient, PromptDispatchOutcome};
use tempfile::TempDir;

use super::helpers::write_acp_stub;

fn spawn_stub_client(stub_environment: &[(String, String)]) -> (TempDir, AcpStdioClient) {
    let temporary = TempDir::new().expect("temporary");
    let stub_path = temporary.path().join("acp_stub.sh");
    write_acp_stub(&stub_path);
    let log_path = temporary.path().join("acp_requests.log");
    let mut environment = vec![("ACP_LOG_FILE".to_string(), log_path.display().to_string())];
    environment.extend(stub_environment.iter().cloned());
    let client = AcpStdioClient::spawn(
        stub_path.display().to_string().as_str(),
        temporary.path(),
        &environment,
        false,
    )
    .expect("spawn stub ACP client");
    (temporary, client)
}

#[test]
fn wait_returns_true_immediately_when_no_prompt_is_in_flight() {
    let (_temporary, client) = spawn_stub_client(&[]);
    assert!(
        client.wait_for_prompt_complete(Duration::from_millis(50)),
        "no prompt submitted yet, so the wait should resolve immediately"
    );
}

#[test]
fn wait_times_out_while_pending_then_resolves_on_completion() {
    // The stub holds the prompt response for one second; the bounded wait must
    // report still-pending within that window and resolve once the response
    // (and the dropped completion signal) arrives.
    let (_temporary, mut client) =
        spawn_stub_client(&[("PROMPT_DELAY_SEC".to_string(), "1".to_string())]);
    client.initialize().expect("initialize stub ACP client");

    let outcome = client.prompt("sess-wait", "status?", None, Box::new(|_completion| {}));
    assert!(
        matches!(outcome, PromptDispatchOutcome::Submitted),
        "prompt dispatch should be accepted, got {outcome:?}"
    );

    assert!(
        !client.wait_for_prompt_complete(Duration::from_millis(50)),
        "the prompt is still in flight during the stub delay"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut completed = false;
    while Instant::now() < deadline {
        if client.wait_for_prompt_complete(Duration::from_millis(100)) {
            completed = true;
            break;
        }
    }
    assert!(completed, "the prompt should complete after the stub delay");

    assert!(
        client.wait_for_prompt_complete(Duration::from_millis(10)),
        "a completed prompt leaves no pending receiver, so the wait resolves immediately"
    );
}
