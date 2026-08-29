use super::*;

#[test]
fn a_long_agent_turn_never_arms_the_execution_watchdog() {
    let temporary = GuardedTempDir::new();
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let options = AcpStubOptions {
        never_respond_to_prompt: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 500,
        fence_observation_timeout_ms: 500,
        ..DeliveryConfiguration::default()
    });

    let result = send_result(dispatch_send(&config_root, &tmux_socket));
    assert_eq!(result.outcome, SendOutcome::Queued);

    // Resolved at the write, while the turn it belongs to is still running.
    let completed = await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        "\"target_session\":\"bravo\"",
    );
    assert!(
        completed.contains("\"outcome\":\"delivered\""),
        "the framed write is the delivery boundary: {completed}"
    );

    // Several multiples of the bound, with the turn still in flight the whole
    // time. An armed watchdog would have fenced this target by now.
    std::thread::sleep(Duration::from_millis(2_500));
    let log = std::fs::read_to_string(&inscriptions).unwrap_or_default();
    assert!(
        !log.contains("relay.delivery.watchdog.armed"),
        "an agent that is slow to answer must not arm the execution watchdog: {log}"
    );
    assert!(
        !log.contains("\"trigger\":\"submission_timeout\""),
        "no fence may be initiated against a healthy target mid-turn: {log}"
    );
}

// Terminal resolutions recorded for one message. Exactly-once is a claim about
// this count, not about the outcome, so it is counted rather than found.
