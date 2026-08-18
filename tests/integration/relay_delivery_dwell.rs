//! The health axis, against a real tmux target the test can make unreachable.
//!
//! The dwell is the one bound that *does* resolve a waiting member, so what
//! matters is that it resolves only sustained unreachability and never a
//! transient one. That half needs a target that changes reachability mid-test,
//! which is why it runs against a real tmux server the test creates and
//! destroys rather than against a permanently-absent socket. The other half here
//! is the axis's *scope*: what health gates and what it does not.

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
/// only means something if the same member is then observed to arrive.
/// `an_unreachable_target_under_the_dwell_is_never_authorized` covers the
/// holding alone; what this one adds is the recovery that gives the holding its
/// purpose.
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

/// Terminal resolutions recorded for one message.
fn completions_for(path: &Path, message_id: &str) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|record| {
            record["event"] == "relay.send.async.completed"
                && record["details"]["message_id"] == message_id
        })
        .count()
}

/// The reason code recorded beside one message's outcome. The two unreachability
/// paths share the `not_submitted` spelling and differ only here, so an
/// assertion about *which* path resolved a member has to read this.
fn reason_code_for(path: &Path, message_id: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| {
            record["event"] == "relay.send.async.completed"
                && record["details"]["message_id"] == message_id
        })
        .and_then(|record| {
            record["details"]["reason_code"]
                .as_str()
                .map(str::to_string)
        })
}

/// The reported outcome for one message, once it has resolved.
fn outcome_for(path: &Path, message_id: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| {
            record["event"] == "relay.send.async.completed"
                && record["details"]["message_id"] == message_id
        })
        .and_then(|record| record["details"]["outcome"].as_str().map(str::to_string))
}

/// Queues one member for `bravo` and returns its message id.
fn send_to_bravo(
    config_root: &agentmux::configuration::ConfigurationRoots,
    runtime_directory: &Path,
    marker: &str,
) -> String {
    let response = handle_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: marker.to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
        config_root,
        "party",
        runtime_directory,
    )
    .expect("async send should be accepted");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results[0].outcome, SendOutcome::Queued);
    results[0].message_id.clone()
}

/// Waits until every listed message has resolved.
fn await_all_resolved(path: &Path, message_ids: &[String], budget: Duration) {
    let started = std::time::Instant::now();
    loop {
        let unresolved: Vec<&String> = message_ids
            .iter()
            .filter(|id| completions_for(path, id.as_str()) == 0)
            .collect();
        if unresolved.is_empty() {
            return;
        }
        assert!(
            started.elapsed() < budget,
            "timed out with unresolved members: {unresolved:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A target flapping across the dwell boundary resolves every member exactly
/// once, whichever side of the boundary each one lands on.
///
/// Uniqueness is the property worth asserting under flapping and the only one
/// that is not a race. Which outcome a given member takes depends on where the
/// reachability transitions fall relative to its own wait, so pinning an outcome
/// for every member would be pinning the scheduler — and a test that flaps a
/// target and then asserts a particular verdict is the flaky shape this one is
/// deliberately not.
///
/// Two members are still driven into determinate corners, so a passing run is
/// known to have crossed the boundary in **both** directions rather than having
/// sat on one side: the third and fourth are sent with no server and left well
/// past the dwell, and the fifth is sent to a live session. Without those, every
/// assertion here would hold equally for a run where nothing ever went
/// unreachable.
///
/// The reachability transitions are driven by creating and destroying a real
/// tmux server rather than by a stub, so what crosses the boundary is the same
/// observation path production uses.
#[test]
fn a_target_flapping_across_the_dwell_resolves_every_member_exactly_once() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    if !tmux_available() {
        eprintln!("skipping flapping test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    // Long enough that the first pair cannot reach it before the server appears,
    // short enough that the third and fourth reach it inside the test.
    configure_delivery(DeliveryConfiguration {
        unreachable_dwell_ms: 1_500,
        ..DeliveryConfiguration::default()
    });

    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    // Unreachable, under the dwell: two members are held.
    let mut sent = vec![
        send_to_bravo(&config_root, &paths.runtime_directory, "FLAP-A"),
        send_to_bravo(&config_root, &paths.runtime_directory, "FLAP-B"),
    ];
    std::thread::sleep(Duration::from_millis(200));

    // First crossing, into reachable. The held pair resolves here.
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");
    await_all_resolved(&inscriptions, &sent, Duration::from_millis(15_000));

    // Second crossing, back out. The pause before sending is what puts these two
    // on the dwell path rather than the pre-flight one: `submit_batch` consults
    // the gate *before* authorization, so a member sent while the cached
    // observation still reads healthy is admitted and then fails at the write
    // with `tmux_target_unavailable`. Waiting for the observer to record the
    // departure first is what makes the gate itself refuse them.
    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
    std::thread::sleep(Duration::from_millis(400));
    let past_dwell = vec![
        send_to_bravo(&config_root, &paths.runtime_directory, "FLAP-C"),
        send_to_bravo(&config_root, &paths.runtime_directory, "FLAP-D"),
    ];
    await_all_resolved(&inscriptions, &past_dwell, Duration::from_millis(15_000));
    for message_id in &past_dwell {
        assert_eq!(
            outcome_for(&inscriptions, message_id.as_str()).as_deref(),
            Some("not_submitted"),
            "a member left past the dwell on an absent server must resolve at the gate"
        );
        assert_eq!(
            reason_code_for(&inscriptions, message_id.as_str()).as_deref(),
            Some("delivery_target_unreachable"),
            "the member resolved through a path other than the dwell, so this run \
             never crossed the boundary it claims to test"
        );
    }

    // Third crossing, back in — and the pause here is load-bearing for the same
    // reason as the one above, in the opposite direction. The dwell has already
    // elapsed, so until the observer's next successful observation clears
    // `unreachable_since`, the gate still reads `Unreachable` and bounces a
    // member sent to a target that is in fact healthy. That is the cost this
    // design states rather than a defect this test found: a target that recovers
    // near the threshold has members bounced that could have been delivered.
    spawn_session(&paths.tmux_socket, "bravo", "exec sleep 45");
    std::thread::sleep(Duration::from_millis(400));
    let after_recovery = send_to_bravo(&config_root, &paths.runtime_directory, "FLAP-E");
    await_all_resolved(
        &inscriptions,
        std::slice::from_ref(&after_recovery),
        Duration::from_millis(15_000),
    );
    assert_eq!(
        outcome_for(&inscriptions, after_recovery.as_str()).as_deref(),
        Some("delivered"),
        "a member sent after the target came back must deliver"
    );

    // The property under flapping: one resolution each, for every member.
    sent.extend(past_dwell);
    sent.push(after_recovery);
    for message_id in &sent {
        assert_eq!(
            completions_for(&inscriptions, message_id.as_str()),
            1,
            "member {message_id} did not resolve exactly once across the flapping"
        );
    }

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}

/// Health gates writes and does not gate `look`: against one unreachable target,
/// a `raww` resolves at the health gate while a `look` still reaches the
/// transport's snapshot implementation.
///
/// The two halves are ordered so the second one means something. The `raww` is
/// driven first and its resolution is *awaited*, because `delivery_target_unreachable`
/// is produced only by `gate_target` past the dwell — so by the time the `look`
/// is issued, the target is known to read `Unreachable` on the health axis
/// rather than merely being expected to. A look issued before that would prove
/// nothing about a gate that had not yet closed.
///
/// `raww` is the interesting write here rather than a send: it is gated by
/// entering the same per-target ordered channel mail uses, not by a rule of its
/// own, and the resolution spelling is what shows it arrived there.
///
/// What "served" reaches for tmux is narrower than the word suggests, and the
/// limit is structural rather than an omission. A tmux transport reports
/// `Unreachable` *because* its pane cannot be observed, and that same absent pane
/// is what the snapshot would be captured from — so no tmux look can return
/// content for a target this axis calls unreachable. What is observable is the
/// error's origin: the look fails inside `TmuxOutputView::look` on its own
/// capture attempt, which is a different thing from being refused before
/// dispatch.
///
/// So the property held here is **not rejected before transport dispatch**, and
/// it is not the stronger "answered": this look ends in an error, and an error
/// from the transport is not a served snapshot. Nothing here shows a look
/// answered with a payload while the health axis reads `Unreachable`.
/// `acp_look_without_startup_returns_unavailable_stale_metadata` is the nearest
/// neighbour and does not show it either — no ACP worker has ever started there,
/// so no transport exists to read health from, and its snapshot is empty rather
/// than content.
///
/// The look's requester is the target itself because the shared fixture's policy
/// scopes `look` to `self`. Requester identity is not part of the property.
#[test]
fn look_survives_the_health_gate_that_holds_raww_on_an_unreachable_target() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    if !tmux_available() {
        eprintln!("skipping look-versus-raww gating test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    // Short enough that the raww half resolves inside the test, which is what
    // establishes the health axis reads `Unreachable` before the look is issued.
    configure_delivery(DeliveryConfiguration {
        unreachable_dwell_ms: 400,
        ..DeliveryConfiguration::default()
    });

    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

    // No tmux server is ever created here: the target is unreachable from the
    // first observation and stays that way.
    let response = handle_request(
        RelayRequest::Raww {
            request_id: Some("req-raww-gated".to_string()),
            requester_session: "alpha".to_string(),
            target_session: "bravo@party".to_string(),
            text: "raw input for an unreachable target".to_string(),
            no_enter: false,
            on_behalf_of: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("raw input should be accepted for asynchronous delivery");
    let RelayResponse::Raww { status, .. } = response else {
        panic!("expected raww response");
    };
    assert_eq!(status, "queued");

    let completed = await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        Duration::from_millis(15_000),
    );
    let record: serde_json::Value =
        serde_json::from_str(completed.as_str()).expect("completed inscription is json");
    assert_eq!(
        record["details"]["reason_code"].as_str(),
        Some("delivery_target_unreachable"),
        "raw input resolves at the health gate, not at a raww-specific refusal: {completed}"
    );

    // The health axis now reads `Unreachable` for this target. A look against it
    // is still dispatched.
    let error = handle_request(
        RelayRequest::Look {
            requester_session: "bravo".to_string(),
            target_session: "bravo@party".to_string(),
            lines: Some(20),
            offset: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect_err("a tmux look cannot capture a pane that does not exist");
    assert_eq!(
        error.message, "failed to resolve active pane for look target",
        "the look reached TmuxOutputView::look and failed on its own capture rather than \
         being refused ahead of the transport: {error:?}"
    );
}
