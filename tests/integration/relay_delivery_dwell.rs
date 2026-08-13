//! The unreachability dwell, against a target whose reachability the test moves.
//!
//! The dwell is the one bound that *does* resolve a waiting member, so what
//! matters is that it resolves only sustained unreachability and never a
//! transient one. Both halves need a target that changes reachability mid-test,
//! which is why these run against a real tmux server the test creates and
//! destroys rather than against a permanently-absent socket.

use std::{path::Path, time::Duration};

use agentmux::{
    relay::{RelayRequest, RelayResponse, SendOutcome, handle_request},
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

use crate::support::relay_delivery::{
    TmuxServerGuard, spawn_session, tmux_available, tmux_command, wait_for_pane_contains,
    write_bundle_configuration,
};

/// Counts inscriptions of one event in the process log.
fn count_inscriptions(path: &Path, event: &str) -> usize {
    let needle = format!("\"event\":\"{event}\"");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(needle.as_str()))
        .count()
}

/// Waits for one inscription of `event`, returning its line.
fn await_inscription(path: &Path, event: &str, budget: Duration) -> String {
    let needle = format!("\"event\":\"{event}\"");
    let started = std::time::Instant::now();
    loop {
        let log = std::fs::read_to_string(path).unwrap_or_default();
        if let Some(line) = log.lines().find(|line| line.contains(needle.as_str())) {
            return line.to_string();
        }
        assert!(
            started.elapsed() < budget,
            "timed out waiting for {event}, log={log}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// An unreachability shorter than the dwell resolves nothing, and the member it
/// was holding still delivers once the target comes back.
///
/// The second half is what makes the first half worth asserting. "Nothing was
/// resolved" is also true of a member that was silently dropped, so the test
/// only means something if the same member is then observed to arrive. An
/// earlier test in this arc covers the holding alone; what was missing is the
/// recovery that gives the holding its purpose.
///
/// The target starts with no server behind its socket at all and becomes
/// available when the test creates one, which is the transition an operator
/// produces by restarting a crashed tmux server — driven for real rather than
/// simulated with a stub.
///
/// What this test does **not** establish is that the health observer saw
/// `Unreachable` before that. It never inspects the observer's state, so an
/// observer that first ran only after the server existed would satisfy every
/// assertion here. The dwell's involvement was confirmed separately, by
/// shortening it to 300ms and watching the member resolve inside the hold
/// window; that check lives in the task narrative rather than in this file,
/// because an in-test discriminator needs an observer-state seam that does not
/// exist yet.
#[test]
fn a_transient_unreachability_resolves_nothing_and_the_member_still_delivers() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    if !tmux_available() {
        eprintln!("skipping dwell-recovery test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    // Long enough that this test cannot reach it. The point is that the member
    // is held by an unreachability that ends *before* the threshold, so a dwell
    // short enough to fire would be measuring the wrong thing entirely.
    configure_delivery(DeliveryConfiguration {
        unreachable_dwell_ms: 600_000,
        ..DeliveryConfiguration::default()
    });

    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    // No tmux server yet: the target is unreachable from the first observation.
    let marker = "DWELL-RECOVERY-MARKER";
    let response = handle_request(
        RelayRequest::Send {
            request_id: Some("req-dwell-recovery".to_string()),
            requester_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(50),
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("async send should be accepted");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Several worker poll ticks of unreachability, far short of the dwell.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        count_inscriptions(&inscriptions, "relay.send.async.completed"),
        0,
        "an unreachability shorter than the dwell must resolve nothing"
    );

    // The target comes back.
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");

    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        marker,
        Duration::from_millis(15_000),
    );
    let completed = await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        Duration::from_millis(15_000),
    );
    let record: serde_json::Value =
        serde_json::from_str(completed.as_str()).expect("completed inscription is json");
    assert_eq!(
        record["details"]["outcome"].as_str(),
        Some("delivered"),
        "the member held through a transient unreachability delivers: {completed}"
    );
    assert_eq!(
        count_inscriptions(&inscriptions, "relay.send.async.completed"),
        1,
        "the member resolves exactly once across the reachability transition"
    );

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}
