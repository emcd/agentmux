//! How a bundle teardown resolves the members it was still holding, and what it
//! is allowed to tell their senders.

use agentmux::relay::RelayRequest;

use super::*;

/// A bundle stop tells the sender its bundle stopped, never that the relay shut
/// down.
///
/// The two endings share a drain, and the shutdown spelling is the one the
/// delivery spec requires for a `Pending` member the relay still held — so the
/// drain defaults to it and a bundle stop inherits a claim that is simply false
/// while the relay carries on serving every other bundle. The held member and
/// the members still sitting in the worker's receiver reach that drain by
/// different routes, which is why both are exercised here: fixing one and not
/// the other is exactly the shape this regressed in.
///
/// The fixture builds the queue deliberately. Member one meets a prompt-ready
/// pane and is handed a paste that never returns, so the worker is occupied; the
/// pane is then reported busy and two more members are admitted behind it, one
/// becoming the loop's single held member and the other waiting in the receiver.
/// The teardown then ends all three.
#[test]
fn a_bundle_stop_is_never_reported_to_a_sender_as_a_relay_shutdown() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};
    use std::time::{Duration, Instant};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let fake_tmux = temporary.path().join("fake-tmux");
    let busy_file = temporary.path().join("target-busy");
    let pasted_file = temporary.path().join("paste-started");
    write_stateful_fake_tmux(&fake_tmux, &busy_file, &pasted_file);
    // SAFETY: nextest runs each test in its own process, and this runs before the
    // first dispatch spawns anything that could read the environment.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    // The submission timeout is held far off so the watchdog cannot fence the
    // blocked member and resolve it under its own trigger, which would take the
    // queue down before the teardown ever runs. The fence observation is
    // shortened instead, because the teardown waits on it and nothing here needs
    // a realistic cessation window.
    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 60_000,
        unreachable_dwell_ms: 600_000,
        fence_observation_timeout_ms: 200,
        ..Default::default()
    });

    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    write_prompt_readiness_coders(&config_root);
    let tmux_socket = temporary.path().join("tmux.sock");

    let send = || {
        dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: "hello".to_string(),
                targets: vec!["bravo@party".to_string()],
                broadcast: false,
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect("send response");
    };

    send();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pasted_file.exists() {
        assert!(
            Instant::now() < deadline,
            "the first member never reached a paste, so the worker is not occupied"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::write(&busy_file, b"1").expect("write busy marker");
    // Readiness is read from a cached observation the transport refreshes on its
    // own 100ms clock, so a send issued immediately after the flip is authorized
    // against the *previous* reading. Without this wait all three members reach a
    // batch and the receiver is empty at teardown — which is what the first draft
    // of this test did, leaving the queued path it exists to cover unexercised.
    std::thread::sleep(Duration::from_millis(400));
    send();
    send();
    std::thread::sleep(Duration::from_millis(250));

    // The fixture's own premise: nothing has resolved yet, so every outcome read
    // below is one the teardown produced.
    assert!(
        read_inscriptions(&inscriptions, "relay.send.async.completed").is_empty(),
        "no member may resolve before the teardown runs"
    );

    agentmux::relay::shutdown_bundle_runtime("party", temporary.path(), &tmux_socket)
        .expect("bundle teardown");

    let completions = read_inscriptions(&inscriptions, "relay.send.async.completed");
    assert_eq!(
        completions.len(),
        3,
        "the teardown resolves every admitted member: {completions:?}"
    );
    let records = completions
        .iter()
        .map(|completion| {
            serde_json::from_str::<serde_json::Value>(completion.as_str())
                .expect("completion is json")
        })
        .collect::<Vec<_>>();

    // The queue shape this test depends on, asserted rather than assumed. One
    // member reached a batch and two did not: the held one and the one still in
    // the worker's receiver, which are the two routes into the drain. The first
    // draft of this fixture authorized all three, leaving the receiver empty, and
    // passed against the defect for that reason alone — so a fixture that stops
    // building the queue must fail here rather than quietly stop testing.
    let unbound = records
        .iter()
        .filter(|record| record["details"]["batch_id"].is_null())
        .count();
    assert_eq!(
        unbound, 2,
        "two members must be waiting unbound, one held and one queued: {records:#?}"
    );

    for record in &records {
        let details = &record["details"];
        assert_ne!(
            details["reason_code"], "dropped_on_shutdown",
            "a bundle stop must not be reported as a relay shutdown: {record}"
        );
        assert_eq!(
            details["reason"], "bundle stopped before this member resolved",
            "every member names the ending that actually happened: {record}"
        );
    }
}
