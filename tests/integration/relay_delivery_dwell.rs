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
