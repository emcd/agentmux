//! Send operation tests: target resolution, broadcast, async dispatch,
//! cross-relay bang-path resolution.

use agentmux::relay::{RelayRequest, RelayResponse, SendOutcome};

use super::*;

#[test]
fn send_rejects_unknown_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["missing@party".to_string()],
            broadcast: false,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("send should fail");
    assert_eq!(response.code, "validation_unknown_target");
}

#[test]
fn send_rejects_target_by_configured_session_name_alias() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["Bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("send should fail");
    assert_eq!(response.code, "validation_unknown_target");
}

#[test]
fn send_accepts_global_ui_target_not_in_bundle_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["user@GLOBAL".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "user@GLOBAL");
}

#[test]
fn send_prefers_bundle_member_when_target_id_overlaps_with_ui_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration_with_session_id(&config_root, "default", "alpha@GLOBAL");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "bravo".to_string(),
            message: "hello".to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "alpha@party");
    // Overlap precedence is bundle-member-first: the resolved target is the
    // bundle member alpha@party, not the overlapping UI session. Async
    // dispatch queues the delivery regardless of tmux runtime presence.
    assert_eq!(results[0].outcome, SendOutcome::Queued);
}

#[test]
fn send_broadcast_excludes_sender_session() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: Vec::new(),
            broadcast: true,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "bravo@party");
}

#[test]
fn send_async_returns_accepted_and_queued_outcome() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].target_session, "bravo@party");
    assert_eq!(results[0].outcome, SendOutcome::Queued);
}

#[test]
fn send_broadcast_with_only_sender_returns_empty_results() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_single_member_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: Vec::new(),
            broadcast: true,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert!(results.is_empty());
}

#[test]
fn send_without_peer_manager_reports_cross_relay_unavailable() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    // A bang-path target is classified as cross-relay from the address alone
    // (config-free). The non-stream single-bundle entry point holds no peer
    // connection manager, so forwarding cannot run and the target reports as
    // unavailable rather than a misleading unknown-bundle error. Real forwarding
    // is exercised through the stream path, which supplies the manager.
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@other!peer-relay".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("cross-relay send has no peer manager on the non-stream path");
    assert_eq!(response.code, "runtime_cross_relay_unavailable");
}

#[test]
fn send_rejects_malformed_cross_relay_target() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    for target in ["bravo@other!", "bravo!peer-relay"] {
        let response = dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: "hello".to_string(),
                targets: vec![target.to_string()],
                broadcast: false,
                quiet_window_ms: Some(1),
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect_err("malformed cross-relay target should fail at resolution");
        assert_eq!(
            response.code, "validation_malformed_cross_relay_target",
            "target {target} should be rejected as malformed",
        );
    }
}

/// An envelope whose canonical payload alone exceeds what the target's transport
/// will ever accept is refused at admission rather than queued, because no
/// partition could ever carry it. The refusal is structured and names both the
/// size and the ceiling so the caller can act on it.
#[test]
fn send_rejects_a_payload_larger_than_the_transport_can_accept() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let oversized = "x".repeat(262_144 + 1);
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: oversized,
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("an oversized payload should be refused at admission");
    assert_eq!(response.code, "validation_payload_too_large");
    let details = response.details.expect("rejection carries details");
    assert_eq!(details["canonical_bytes"], 262_145);
    assert_eq!(details["canonical_bytes_max"], 262_144);
}

/// A payload at exactly the transport's ceiling is admitted: the bound is the
/// largest accepted size, not the smallest rejected one. Without this the
/// oversize test above passes against an off-by-one that refuses valid sends.
#[test]
fn send_admits_a_payload_at_exactly_the_transport_ceiling() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "x".repeat(262_144),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("a payload at the ceiling is admitted");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);
}

/// A Pubsub target is refused synchronously at admission. The request fails as a
/// whole rather than returning `queued` and resolving later, so no work is
/// authorized merely to discover the forward-declared stub.
#[test]
fn send_rejects_a_pubsub_target_at_admission() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle_with_policy(
        &temporary,
        "party",
        r#"
format-version = 1

[[sessions]]
id = "alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "feed"
directory = "/tmp"

[sessions.pubsub]
"#,
        Some(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["feed@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("a pubsub target should be refused at admission");
    assert_eq!(response.code, "runtime_session_type_not_implemented");
}

/// A request naming a Pubsub target alongside a deliverable one is refused
/// whole: admission reserves for every target before queueing any, so the
/// deliverable target is not left queued behind a refused request.
///
/// The absence of the deliverable target's `queued` inscription is the claim, so
/// the test first drives a send that does produce one. Without that control the
/// absence assertion would pass just as well against inscriptions that never
/// reached this file at all.
#[test]
fn send_rejecting_one_target_queues_none_of_the_request() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle_with_policy(
        &temporary,
        "party",
        r#"
format-version = 1

[[sessions]]
id = "alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "bravo"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "feed"
directory = "/tmp"

[sessions.pubsub]
"#,
        Some(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"
"#,
        ),
    );
    let tmux_socket = temporary.path().join("tmux.sock");

    // Control: a send the relay does accept queues bravo and says so.
    dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");
    let queued_before = count_bravo_queued_inscriptions(&inscriptions);
    assert_eq!(
        queued_before, 1,
        "control send should queue bravo exactly once"
    );

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string(), "feed@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect_err("a request naming a pubsub target is refused whole");
    assert_eq!(response.code, "runtime_session_type_not_implemented");
    assert_eq!(
        count_bravo_queued_inscriptions(&inscriptions),
        queued_before,
        "the refused request must not have queued its deliverable target"
    );
}

/// Counts `relay.send.async.queued` inscriptions naming bravo. The `queued`
/// inscription is written synchronously before the send response returns, so its
/// absence after a refused request is decidable at that point.
fn count_bravo_queued_inscriptions(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            line.contains("\"event\":\"relay.send.async.queued\"")
                && line.contains("\"target_session\":\"bravo\"")
        })
        .count()
}

/// Undelivered means *waiting*, not merely reserved.
///
/// An `Authorized` member has been handed to its transport and is executing
/// under the watchdog's bound. Counting it would report work in progress as a
/// backlog, and would age it toward a warning that says a target is not draining
/// while it is in fact being written to.
///
/// This asserts the scoping rule rather than concrete queue depths, because the
/// relay currently authorizes a member the instant its worker receives it — so
/// almost nothing stays `Pending` long enough to count. The count and dedup
/// assertions return once authorization is gated on readiness; see
/// `agentmux:todos/relay/130`.
#[test]
fn undelivered_reporting_counts_pending_entries_and_not_authorized_ones() {
    use agentmux::relay::{UndeliveredReporting, report_undelivered_queue};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");
    let reporting = UndeliveredReporting::default();

    // Nothing admitted yet: the aggregate is suppressed.
    report_undelivered_queue(reporting);
    assert_eq!(
        count_inscriptions(&inscriptions, "relay.delivery.undelivered"),
        0,
        "an idle relay emits no aggregate"
    );

    dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["user@GLOBAL".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    // The member is admitted and holds quota, but its worker authorizes it
    // immediately, so it is executing rather than waiting. A report that counted
    // reservations would name it here; one scoped to `Pending` does not.
    std::thread::sleep(std::time::Duration::from_millis(300));
    report_undelivered_queue(reporting);
    let aggregates = read_inscriptions(&inscriptions, "relay.delivery.undelivered");
    assert!(
        aggregates
            .iter()
            .all(|line| !line.contains("\"target_session\":\"user@GLOBAL\"")),
        "an authorized member is executing, not backlogged, so it is not reported \
         as undelivered: {aggregates:?}"
    );
}

/// A target with nothing `Pending` produces no warning, however much quota its
/// executing members still hold.
///
/// The per-target dedup this once asserted — three entries crossing a threshold
/// together warning once rather than once each — needs entries that stay
/// `Pending`, which the relay cannot currently produce because it authorizes on
/// receipt. What survives is the half that is still reachable: a warning is a
/// statement about a target that is *not draining*, so members that have been
/// handed over must not produce one. See `agentmux:todos/relay/130` for
/// restoring the dedup and count assertions.
#[test]
fn an_authorized_member_produces_no_undelivered_warning() {
    use agentmux::relay::{UndeliveredReporting, report_undelivered_queue};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

    for _ in 0..3 {
        dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: "hello".to_string(),
                targets: vec!["user@GLOBAL".to_string()],
                broadcast: false,
                quiet_window_ms: Some(1),
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect("send response");
    }
    std::thread::sleep(std::time::Duration::from_millis(300));

    // A zero threshold makes any `Pending` entry already past it, so this is the
    // most permissive possible condition for emitting a warning. Nothing is
    // emitted anyway, because none of these members is waiting.
    let reporting = UndeliveredReporting {
        warning: std::time::Duration::ZERO,
        ..UndeliveredReporting::default()
    };
    report_undelivered_queue(reporting);
    let warnings = read_inscriptions(&inscriptions, "relay.delivery.undelivered.warning");
    assert!(
        warnings
            .iter()
            .all(|line| !line.contains("\"target_session\":\"user@GLOBAL\"")),
        "a warning names a target that is not draining; these members were handed \
         over and are executing: {warnings:?}"
    );
}

/// The `[delivery]` table has to reach the reservation, not merely parse.
///
/// Publishing a per-target envelope quota of one and sending twice to a target
/// that stays queued separates the two: with the configured value in force the
/// second send is refused, and with only the compiled-in default in force it is
/// accepted like the first. The refusal names the configured limit rather than
/// the default, so the assertion cannot pass against the wrong bound.
#[test]
fn a_configured_quota_binds_at_admission() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        queued_envelopes_per_target_max: 1,
        ..DeliveryConfiguration::default()
    });

    let send = || {
        dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: "hello".to_string(),
                targets: vec!["user@GLOBAL".to_string()],
                broadcast: false,
                quiet_window_ms: Some(1),
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
    };

    // The UI target holds its entry through the reconnect wait, so the first
    // reservation is still live when the second send is admitted.
    send().expect("the first send fits the configured per-target quota");
    let error = send().expect_err("the second send exceeds a per-target quota of one");

    assert_eq!(error.code, "runtime_delivery_queue_full");
    let details = error.details.as_ref().expect("queue-full details");
    assert_eq!(
        details
            .get("queued_envelopes_per_target_max")
            .and_then(serde_json::Value::as_u64),
        Some(1),
        "the refusal must name the configured limit, not the compiled-in default: {details}"
    );
    assert_eq!(
        details.get("scope").and_then(serde_json::Value::as_str),
        Some("target"),
    );
}

/// Authorization mints a batch and an attempt id for every member and binds them
/// to the entry, so the terminal record can name the authorization a delivery
/// resolved under.
///
/// Without those identities on the wire the state model would be internal
/// bookkeeping no operator could correlate — a stuck target's outcome could not
/// be traced back to the authorization that produced it.
#[test]
fn a_terminal_outcome_names_the_authorization_it_resolved_under() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let completed = await_inscription(&inscriptions, "relay.send.async.completed");
    let record: serde_json::Value =
        serde_json::from_str(completed.as_str()).expect("completed inscription is json");
    let payload = record
        .get("details")
        .expect("completed inscription carries a details object");
    assert!(
        payload
            .get("batch_id")
            .and_then(serde_json::Value::as_u64)
            .is_some(),
        "an authorized member's terminal record carries its batch id: {completed}"
    );
    assert!(
        payload
            .get("attempt_id")
            .and_then(serde_json::Value::as_u64)
            .is_some(),
        "an authorized member's terminal record carries its attempt id: {completed}"
    );
}

/// One member, one terminal record. The guard's compare-and-swap is what makes
/// that a property rather than a consequence of only one path happening to run.
#[test]
fn a_member_produces_exactly_one_terminal_record() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

    dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    await_inscription(&inscriptions, "relay.send.async.completed");
    // Settle past the resolution before counting, so a second record produced by
    // a losing path would have been written by the time the count is taken.
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        count_inscriptions(&inscriptions, "relay.send.async.completed"),
        1,
        "exactly one terminal record per member"
    );
}

/// The post-authorization execution watchdog, end to end.
///
/// A UI target with no UI connected holds its member for the full 30s reconnect
/// wait, which is far longer than any bound the relay allows its own execution.
/// With the watchdog armed, that member does not wait: the bound elapses, the
/// relay initiates the generation fence, the fence's cooperative stop reaches
/// the UI executor, and the member resolves through the guard's evidence order.
///
/// The outcome is the load-bearing assertion. `not_submitted` is a *sound
/// assertion of non-delivery* produced by the transport proving it emitted
/// nothing — not a timeout spelling, and not an inference from elapsed time. An
/// elapsed watchdog says our execution overran the time we allow it; it says
/// nothing about the target, which is what separates this bound from the absence
/// timers this change retires.
#[test]
fn the_execution_watchdog_fences_an_overrunning_member_instead_of_waiting() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 500,
        fence_observation_timeout_ms: 100,
        ..DeliveryConfiguration::default()
    });

    let started = std::time::Instant::now();
    dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["user@GLOBAL".to_string()],
            broadcast: false,
            quiet_window_ms: Some(1),
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let completed = await_inscription(&inscriptions, "relay.send.async.completed");
    let elapsed = started.elapsed();

    assert!(
        completed.contains("\"outcome\":\"not_submitted\""),
        "the fenced member resolves from evidence, not a timeout spelling: {completed}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the watchdog must resolve this well inside the 30s UI reconnect wait: {elapsed:?}"
    );
    assert_eq!(
        count_inscriptions(&inscriptions, "relay.delivery.watchdog.elapsed"),
        1,
        "the bound elapsing is recorded once for this target"
    );
    let verdict = await_inscription(&inscriptions, "relay.delivery.fence.verdict");
    assert!(
        verdict.contains("\"verdict\":\"positive\""),
        "an executor that observes the fenced flag ceases and the fence goes positive: {verdict}"
    );
    assert!(
        verdict.contains("\"resolution\":\"cooperative\""),
        "it ceased on the cooperative request, so forced termination never ran: {verdict}"
    );
}

/// A fence verdict ends the generation, so the next send gets a new one.
///
/// This is the assertion that bites on resuming a stopped transport. The worker
/// holds one transport for its lifetime, so "a new generation" is observable as
/// *a new worker doing the whole thing again*: a second send that lands on the
/// old, already-fenced transport would be refused by that transport's own fenced
/// flag within a poll interval and would therefore produce no watchdog and no
/// verdict of its own. Seeing a second matched pair is what proves the second
/// send ran the full reconnect wait against a transport that had never been
/// fenced.
///
/// Deliberately positive-verdict: a negative verdict closes the target at the
/// enqueue gate, so it could not distinguish sealing from refusal.
#[test]
fn a_fence_verdict_ends_the_generation_and_the_next_send_gets_a_new_one() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 500,
        fence_observation_timeout_ms: 100,
        ..DeliveryConfiguration::default()
    });

    let send = |body: &str| {
        dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: body.to_string(),
                targets: vec!["user@GLOBAL".to_string()],
                broadcast: false,
                quiet_window_ms: Some(1),
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
    };

    send("first").expect("first send response");
    let first_verdict = await_inscription(&inscriptions, "relay.delivery.fence.verdict");
    assert!(
        first_verdict.contains("\"verdict\":\"positive\""),
        "the UI executor observes the fenced flag and ceases: {first_verdict}"
    );

    // The target is not fenced negative, so it still admits work — through a
    // replacement generation, which is the whole point.
    send("second").expect("second send accepted after a positive verdict");

    await_inscription_count(&inscriptions, "relay.delivery.watchdog.elapsed", 2);
    await_inscription_count(&inscriptions, "relay.delivery.fence.verdict", 2);
    assert_eq!(
        count_inscriptions(&inscriptions, "relay.send.async.completed"),
        2,
        "one terminal record per member, and no more"
    );
}

/// Polls until at least `count` inscription lines exist for `event`.
fn await_inscription_count(path: &std::path::Path, event: &str, count: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let seen = count_inscriptions(path, event);
        if seen >= count {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "only {seen} of {count} {event} inscriptions within 5s; log:\n{}",
            std::fs::read_to_string(path).unwrap_or_default()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Polls for the first inscription line for `event`. The terminal record is
/// written by the delivery worker task rather than on the request path, so it is
/// not present when the send response returns.
fn await_inscription(path: &std::path::Path, event: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(line) = read_inscriptions(path, event).into_iter().next() {
            return line;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {event} inscription within 5s"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Counts inscription lines for exactly `event`. The aggregate and warning event
/// names share a prefix, so matching on the closing quote keeps the aggregate
/// count from absorbing warnings.
fn count_inscriptions(path: &std::path::Path, event: &str) -> usize {
    read_inscriptions(path, event).len()
}

fn read_inscriptions(path: &std::path::Path, event: &str) -> Vec<String> {
    let needle = format!("\"event\":\"{event}\"");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(needle.as_str()))
        .map(str::to_string)
        .collect()
}
