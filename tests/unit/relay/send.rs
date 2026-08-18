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
/// Both halves are asserted against one report, so the counts and the exclusion
/// cannot be satisfied by different states of the queue. `bravo@party` is a tmux
/// target with no server behind its socket: it is unreachable from the first
/// observation, and under a long dwell its members are held rather than
/// resolved, which is a `Pending` that lasts as long as the test needs.
/// `user@GLOBAL` is the UI target, which is always ready and always healthy, so
/// its member authorizes immediately and stays in flight for the reconnect
/// timeout.
#[test]
fn undelivered_reporting_counts_pending_entries_and_not_authorized_ones() {
    use agentmux::relay::{UndeliveredReporting, report_undelivered_queue};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    super::configure_long_unreachable_dwell();
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

    let send = |target: &str| {
        dispatch_request(
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
        .expect("send response");
    };
    for _ in 0..3 {
        send("bravo@party");
    }
    send("user@GLOBAL");

    // Several worker poll ticks: long enough for every member to have reached
    // the state it will hold, far short of the dwell.
    std::thread::sleep(std::time::Duration::from_millis(600));
    report_undelivered_queue(reporting);

    // Teeth for the count: none of the three tmux members resolved during the
    // window, so three is the depth of a queue that is still waiting rather than
    // what a partial drain happened to leave behind.
    //
    // The UI member does resolve, and that is the fourth send's whole purpose
    // now. It reports ready unconditionally and its broadcast finds no
    // subscriber, so it terminalizes at once — which is exactly why the
    // aggregate below counts three and not four. A resolved member is not an
    // undelivered one.
    let completions = read_inscriptions(&inscriptions, "relay.send.async.completed");
    assert_eq!(
        completions.len(),
        1,
        "only the UI member resolves inside the dwell: {completions:?}"
    );
    assert!(
        !completions[0].contains("bravo"),
        "no member held on an unreachable tmux target may resolve here: {completions:?}"
    );

    let aggregates = read_inscriptions(&inscriptions, "relay.delivery.undelivered");
    assert_eq!(aggregates.len(), 1, "one pass emits one aggregate");
    let record: serde_json::Value =
        serde_json::from_str(aggregates[0].as_str()).expect("aggregate is json");
    let details = record.get("details").expect("aggregate carries details");
    assert_eq!(
        details
            .get("undelivered_envelopes_total")
            .and_then(serde_json::Value::as_u64),
        Some(3),
        "the three held members are the whole backlog: {}",
        aggregates[0]
    );
    assert_eq!(
        details
            .get("target_total")
            .and_then(serde_json::Value::as_u64),
        Some(1),
        "the authorized member's target is not a backlogged target: {}",
        aggregates[0]
    );
    let targets = details
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .expect("aggregate carries a targets array");
    assert_eq!(
        targets[0]
            .get("target_session")
            .and_then(serde_json::Value::as_str),
        Some("bravo"),
        "the reported target is the one whose members are waiting: {}",
        aggregates[0]
    );
    assert_eq!(
        targets[0]
            .get("undelivered_envelopes")
            .and_then(serde_json::Value::as_u64),
        Some(3),
        "all three held members are counted against their target: {}",
        aggregates[0]
    );
    // An `Authorized` member is executing, not backlogged. A report that counted
    // reservations rather than waiting would name it here.
    assert!(
        !aggregates[0].contains("\"target_session\":\"user@GLOBAL\""),
        "an authorized member is not reported as undelivered: {}",
        aggregates[0]
    );
}

/// A warning is a condition an operator acts on, not a per-message notification.
///
/// Three entries crossing the threshold together warn once rather than once
/// each, and that suppression persists across passes — the warned flag exists
/// precisely to stop a recurring report from re-announcing a condition already
/// announced. The aggregate carries no such flag and repeats every pass, which
/// is what makes it usable for watching a queue move. Asserting both against the
/// same two passes is what separates "deduplicated" from "emitted once because
/// nothing was reported at all".
#[test]
fn a_backlogged_target_warns_once_while_the_aggregate_repeats() {
    use agentmux::relay::{UndeliveredReporting, report_undelivered_queue};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    super::configure_long_unreachable_dwell();
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

    let send = |target: &str| {
        dispatch_request(
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
        .expect("send response");
    };
    for _ in 0..3 {
        send("bravo@party");
    }
    send("user@GLOBAL");
    std::thread::sleep(std::time::Duration::from_millis(600));

    // A zero threshold makes any `Pending` entry already past it, so every
    // warning this run withholds is withheld by the dedup rather than by the
    // clock.
    let reporting = UndeliveredReporting {
        warning: std::time::Duration::ZERO,
        ..UndeliveredReporting::default()
    };
    report_undelivered_queue(reporting);
    report_undelivered_queue(reporting);

    let warnings = read_inscriptions(&inscriptions, "relay.delivery.undelivered.warning");
    assert_eq!(
        warnings.len(),
        1,
        "one warning covers a target's whole backlog and does not repeat: {warnings:?}"
    );
    let record: serde_json::Value =
        serde_json::from_str(warnings[0].as_str()).expect("warning is json");
    let details = record.get("details").expect("warning carries details");
    assert_eq!(
        details
            .get("target_session")
            .and_then(serde_json::Value::as_str),
        Some("bravo"),
        "the warning names the target that is not draining: {}",
        warnings[0]
    );
    assert_eq!(
        details
            .get("undelivered_envelopes")
            .and_then(serde_json::Value::as_u64),
        Some(3),
        "the warning carries the full waiting count, not one entry's worth: {}",
        warnings[0]
    );
    // A warning names a target that is not draining. The UI member was handed
    // over and is executing, so its target has nothing waiting to warn about —
    // and with the threshold at zero, nothing but the scoping rule suppresses it.
    assert!(
        !warnings[0].contains("\"target_session\":\"user@GLOBAL\""),
        "an authorized member produces no warning: {}",
        warnings[0]
    );

    assert_eq!(
        count_inscriptions(&inscriptions, "relay.delivery.undelivered"),
        2,
        "the aggregate repeats every pass while the backlog stands"
    );
}

/// Fake tmux with two switchable behaviours, addressed through sidecar files
/// rather than arguments so a test can change what the target reports without
/// restarting anything.
///
/// Substituted with `replace` rather than `format!`: the body is almost entirely
/// `${...}` expansions, and doubling every brace for the formatter would bury
/// the shell this fixture is actually made of.
const STATEFUL_FAKE_TMUX: &str = r##"#!/usr/bin/env bash
set -euo pipefail

BUSY_FILE="@BUSY_FILE@"
PASTED_FILE="@PASTED_FILE@"

args=("$@")
if [[ "${#args[@]}" -ge 2 && "${args[0]}" == "-S" ]]; then
  args=("${args[@]:2}")
fi
if [[ "${#args[@]}" -eq 0 ]]; then
  exit 1
fi

case "${args[0]}" in
  display-message)
    case "${args[4]-}" in
      '#{pane_id}')
        printf "%%1\n"
        ;;
      '#{window_activity}')
        printf "1\n"
        ;;
      *)
        printf "\n"
        ;;
    esac
    ;;
  capture-pane)
    if [[ -f "${BUSY_FILE}" ]]; then
      printf "agent is working\n"
    else
      printf "READY-FOR-HANDOVER\n"
    fi
    ;;
  load-buffer)
    cat - > /dev/null
    ;;
  paste-buffer)
    printf "1\n" > "${PASTED_FILE}"
    sleep 60
    ;;
  *)
    :
    ;;
esac
"##;

/// Writes the fake tmux this fixture drives, and returns nothing: every knob it
/// has is a file path the caller already holds.
///
/// Three behaviours, each load-bearing:
///
/// - **`capture-pane` reports a prompt until `busy_file` exists.** That is the
///   readiness axis, and flipping it is what holds later members `Pending`. It is
///   used rather than the activity marker because readiness is read from a cached
///   observation the transport refreshes on its own clock: an advancing marker
///   would only suppress a handover when two gate reads happened to straddle an
///   observer poll, which is a race, while an unready pane suppresses every read
///   after the flip.
/// - **`paste-buffer` never returns.** The relay hands one member over and then
///   waits, so that member stays `Authorized` for as long as the test needs. The
///   sleep is bounded rather than infinite so the orphan it leaves reaps itself;
///   nothing in the test outlives it.
/// - **`display-message` answers a fixed pane and a constant activity marker.**
///   A constant can never advance, so the activity axis never suppresses a
///   handover and the readiness flip above is the only thing that does.
fn write_stateful_fake_tmux(
    script_path: &std::path::Path,
    busy_file: &std::path::Path,
    pasted_file: &std::path::Path,
) {
    use std::os::unix::fs::PermissionsExt;

    let body = STATEFUL_FAKE_TMUX
        .replace(
            "@BUSY_FILE@",
            busy_file.to_str().expect("busy path is utf-8"),
        )
        .replace(
            "@PASTED_FILE@",
            pasted_file.to_str().expect("pasted path is utf-8"),
        );
    std::fs::write(script_path, body).expect("write stateful fake tmux");
    std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set stateful fake tmux executable");
}

/// Republishes the bundle's coder with a prompt-readiness template.
///
/// Without one the transport reports ready whenever the pane can be captured at
/// all, so the fake tmux above would have no way to say "reachable, but not at a
/// prompt" — the state that separates a held member from an unreachable one.
fn write_prompt_readiness_coders(configuration_roots: &ConfigurationRoots) {
    std::fs::write(
        configuration_roots.base_layer().join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
prompt-regex = '^READY-FOR-HANDOVER$'
"#,
    )
    .expect("write prompt-readiness coders file");
}

/// A warning counts what is *waiting* and ages from the oldest of those, not
/// from the reservation ledger.
///
/// The two disagree on exactly one shape: a target holding an `Authorized`
/// member and `Pending` members at the same time. `per_target` is incremented at
/// admission and decremented at release, so it counts the member being written
/// to right now; the waiting tally does not. Every other test in this file leaves
/// them equal, which is why the fix they cover passes with either reading.
///
/// The fixture builds that shape deliberately. Member one meets a prompt-ready
/// pane, is authorized, and is handed a paste that never returns — so it holds
/// its reservation without ever leaving flight. The pane is then reported busy,
/// and members two and three are admitted behind it: reachable target, no
/// prompt, so the gate holds them and they wait.
///
/// Both halves are read off one report, and the aging half is separated by
/// construction rather than by coincidence — the authorized member is aged past
/// the bound the assertion uses before the waiting ones are even sent, so a
/// report measuring from it cannot land under that bound however the machine is
/// scheduled.
#[test]
fn a_warning_counts_the_waiting_members_and_ages_from_the_oldest_of_them() {
    use agentmux::relay::{UndeliveredReporting, report_undelivered_queue};
    use std::time::{Duration, Instant};

    /// How far the authorized member is aged past the waiting ones. Also the
    /// bound the age assertion uses: a report reading the authorized member is
    /// at least this old by construction, and one reading the waiting members
    /// is younger than the settle below.
    const AGE_GAP_MS: u64 = 2_000;

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let fake_tmux = temporary.path().join("fake-tmux");
    let busy_file = temporary.path().join("target-busy");
    let pasted_file = temporary.path().join("paste-started");
    write_stateful_fake_tmux(&fake_tmux, &busy_file, &pasted_file);
    // SAFETY: nextest runs each test in its own process, and this runs before
    // the first dispatch spawns anything that could read the environment.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    // The submission timeout is the one that matters here: at its five-second
    // default the watchdog would fence the blocked member mid-test and
    // terminalize the very reservation the mix depends on. The dwell is
    // lengthened alongside it so a stray unobservable moment cannot resolve a
    // waiting member either.
    agentmux::relay::configure_delivery(agentmux::relay::DeliveryConfiguration {
        submission_timeout_ms: 60_000,
        unreachable_dwell_ms: 600_000,
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
                quiet_window_ms: Some(1),
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect("send response");
    };

    let first_admitted = Instant::now();
    send();
    // The paste marker is the fixture's own proof: the relay authorized this
    // member and handed it to a write that will not return, so it holds an
    // `Authorized` reservation for the rest of the test.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pasted_file.exists() {
        assert!(
            Instant::now() < deadline,
            "the first member never reached a paste, so no member is authorized"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // From here the pane is reachable but not at a prompt, so every later member
    // is held at the gate instead of being authorized behind the first.
    std::fs::write(&busy_file, b"1").expect("write busy marker");
    while first_admitted.elapsed() < Duration::from_millis(AGE_GAP_MS) {
        std::thread::sleep(Duration::from_millis(25));
    }

    send();
    send();
    // Several worker ticks: long enough for both members to have been offered to
    // the gate and held, and short enough that their age stays far below the gap
    // above.
    std::thread::sleep(Duration::from_millis(250));

    // A zero threshold puts every waiting entry past it, so what the warning
    // reports is decided by the scoping rule alone.
    report_undelivered_queue(UndeliveredReporting {
        warning: Duration::ZERO,
        ..UndeliveredReporting::default()
    });

    // The ledger state the assertions below rest on, established rather than
    // assumed: three members admitted against one target, none of them resolved,
    // and exactly one of them authorized. Three live entries, one `Authorized`
    // and two `Pending`.
    assert_eq!(
        count_bravo_queued_inscriptions(&inscriptions),
        3,
        "all three members must be admitted against the one target"
    );
    let completions = read_inscriptions(&inscriptions, "relay.send.async.completed");
    assert!(
        completions.is_empty(),
        "no member may resolve while the paste is still in flight: {completions:?}"
    );
    let authorizations = read_inscriptions(&inscriptions, "relay.delivery.batch.authorized");
    assert_eq!(
        authorizations.len(),
        1,
        "only the member that met a prompt may be authorized: {authorizations:?}"
    );

    let warnings = read_inscriptions(&inscriptions, "relay.delivery.undelivered.warning");
    assert_eq!(warnings.len(), 1, "one backlogged target warns once");
    let record: serde_json::Value =
        serde_json::from_str(warnings[0].as_str()).expect("warning is json");
    let details = record.get("details").expect("warning carries details");
    assert_eq!(
        details
            .get("target_session")
            .and_then(serde_json::Value::as_str),
        Some("bravo"),
    );
    // Reading `per_target` here would say three, because the authorized member
    // still holds its reservation. It is being written to, not backlogged.
    assert_eq!(
        details
            .get("undelivered_envelopes")
            .and_then(serde_json::Value::as_u64),
        Some(2),
        "the warning carries the waiting count, not the reserved one: {}",
        warnings[0]
    );
    // The same exclusion on the aging axis. The authorized member is the oldest
    // entry this target has, so a report that aged from it would announce a
    // target as backlogged since before either waiting member existed.
    let oldest_age_ms = details
        .get("oldest_age_ms")
        .and_then(serde_json::Value::as_u64)
        .expect("warning carries oldest_age_ms");
    assert!(
        oldest_age_ms < AGE_GAP_MS,
        "the warning ages from the oldest waiting entry, not from the authorized one: {}",
        warnings[0]
    );

    // The aggregate is read beside the warning so the two cannot be satisfied by
    // different states of the queue.
    let aggregates = read_inscriptions(&inscriptions, "relay.delivery.undelivered");
    assert_eq!(aggregates.len(), 1, "one pass emits one aggregate");
    let record: serde_json::Value =
        serde_json::from_str(aggregates[0].as_str()).expect("aggregate is json");
    assert_eq!(
        record
            .get("details")
            .and_then(|details| details.get("undelivered_envelopes_total"))
            .and_then(serde_json::Value::as_u64),
        Some(2),
        "the aggregate counts the same two waiting members: {}",
        aggregates[0]
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
                targets: vec!["bravo@party".to_string()],
                broadcast: false,
                quiet_window_ms: Some(1),
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
    };

    // A tmux target with no server behind it is held rather than resolved — it
    // is unreachable, but not yet for the dwell — so the first reservation is
    // still live when the second send is admitted. The UI target will not do
    // here: it reports ready unconditionally and its delivery now resolves from
    // one broadcast attempt, so its reservation is released before the second
    // send arrives.
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
///
/// The target is the UI session rather than a tmux one, and that is load-bearing
/// now that authorization is gated. A tmux target with no server behind it is
/// unreachable, so its member resolves during `Pending` and never mints a batch
/// at all — a correct outcome that simply is not the one under test. UI reports
/// healthy and ready unconditionally, so it reaches the authorized state this
/// test is about.
#[test]
fn a_terminal_outcome_names_the_authorization_it_resolved_under() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

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

    // The UI transport resolves only after its own reconnect wait, and no UI is
    // connected here, so the bound is that wait rather than anything relay-side.
    let completed = await_inscription_within(
        &inscriptions,
        "relay.send.async.completed",
        std::time::Duration::from_secs(45),
    );
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

/// The partition reaches the log, naming the unit and the members bound to it.
///
/// Every other step of a delivery already left a record; which members shared a
/// fate did not. That is the step that decides whose outcome is derived from
/// whose evidence, so without it a reader could see two members resolve
/// identically and be unable to tell whether one record answered for both or two
/// records happened to agree.
///
/// The target is the UI session for the same reason the batch-identity test above
/// uses one: UI reports healthy and ready unconditionally, so it reaches the
/// declaration, while a tmux target with no server behind it resolves during
/// `Pending` and never declares anything.
///
/// A singleton is what this can assert deterministically. A multi-member unit is
/// reachable — the tmux transport drains its channel and declares over the whole
/// coalesced group — but only opportunistically: the drain takes what is
/// *immediately available* and flushes when the channel is empty, with no
/// coalesce wait. So whether a second envelope joins the group is a race, and a
/// test asserting it would be flaky rather than strict.
#[test]
fn a_declared_partition_names_its_unit_and_members() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

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

    let declared = await_inscription_within(
        &inscriptions,
        "relay.delivery.partition.declared",
        std::time::Duration::from_secs(45),
    );
    let record: serde_json::Value =
        serde_json::from_str(declared.as_str()).expect("declaration inscription is json");
    let payload = record
        .get("details")
        .expect("declaration carries a details object");

    assert!(
        payload
            .get("unit_id")
            .and_then(serde_json::Value::as_u64)
            .is_some(),
        "a declaration names the unit it minted: {declared}"
    );
    assert_eq!(
        payload
            .get("member_count")
            .and_then(serde_json::Value::as_u64),
        Some(1),
        "a UI handover is one envelope, so its unit has one member: {declared}"
    );
    // The member ids are the point: a count alone would not say *which* members
    // were bound, which is the only thing that makes a shared outcome auditable.
    let members = payload
        .get("member_ids")
        .and_then(serde_json::Value::as_array)
        .expect("a declaration names its members");
    assert_eq!(members.len(), 1, "member_ids agrees with member_count");
    assert!(
        members[0].as_str().is_some_and(|id| !id.is_empty()),
        "the bound member is named: {declared}"
    );
}

/// The batch reaches the log ahead of the partition, naming the members the relay
/// committed to at one instant.
///
/// Two claims, and the second is the one worth the test. Naming the batch closes
/// the antecedent of every per-member attribution downstream: the partition says
/// which members shared a *submission*, and only the batch says which the relay
/// *authorized together* — a reader with just the partition cannot tell a batch
/// that was split by a transport from two batches that happened to be adjacent.
///
/// The ordering is the contract, not an artifact of how the code happens to run
/// today. Authorization is the linearization point, so a partition recorded ahead
/// of it would mean a member had been bound to a unit — the point past which
/// non-delivery can no longer be proven — before the relay had committed to
/// delivering it at all. Asserting the order here is what makes that
/// falsifiable from outside the relay.
///
/// A singleton, and for a sharper reason than the partition test's: the batch is
/// bounded by what one invocation of the delivery seam carries, and `mailw`
/// carries one envelope. Multi-member batches wait on that seam, so there is no
/// load this test could apply that would produce one.
#[test]
fn an_authorized_batch_precedes_the_partition_it_covers() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

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

    let authorized = await_inscription_within(
        &inscriptions,
        "relay.delivery.batch.authorized",
        std::time::Duration::from_secs(45),
    );
    // Awaited separately so the ordering read below is against a log that has
    // both, rather than one that has only reached the first.
    await_inscription_within(
        &inscriptions,
        "relay.delivery.partition.declared",
        std::time::Duration::from_secs(45),
    );

    let record: serde_json::Value =
        serde_json::from_str(authorized.as_str()).expect("batch inscription is json");
    let payload = record
        .get("details")
        .expect("the batch carries a details object");

    assert!(
        payload
            .get("batch_id")
            .and_then(serde_json::Value::as_u64)
            .is_some(),
        "an authorization names the batch it minted: {authorized}"
    );
    let members = payload
        .get("member_ids")
        .and_then(serde_json::Value::as_array)
        .expect("an authorization names its members");
    assert_eq!(
        payload
            .get("member_count")
            .and_then(serde_json::Value::as_u64),
        Some(members.len() as u64),
        "member_count agrees with member_ids: {authorized}"
    );
    let member = members
        .first()
        .and_then(serde_json::Value::as_str)
        .expect("the authorized member is named");
    assert!(!member.is_empty(), "the authorized member is named");

    // The same member, and the batch first. Read the whole log rather than
    // relying on the two awaits above, because each returns as soon as its own
    // event appears and neither says which was written first.
    let log = std::fs::read_to_string(&inscriptions).expect("inscriptions readable");
    let position = |event: &str| {
        log.lines()
            .position(|line| line.contains(format!("\"event\":\"{event}\"").as_str()))
            .unwrap_or_else(|| panic!("no {event} in the log: {log}"))
    };
    assert!(
        position("relay.delivery.batch.authorized") < position("relay.delivery.partition.declared"),
        "the batch is authorized before its partition is declared: {log}"
    );
    let partition = log
        .lines()
        .find(|line| line.contains("\"event\":\"relay.delivery.partition.declared\""))
        .expect("the partition is in the log");
    assert!(
        partition.contains(member),
        "the partition covers the member the batch authorized: {partition}"
    );
}

/// One member, one terminal record. The guard's compare-and-swap is what makes
/// that a property rather than a consequence of only one path happening to run.
#[test]
fn a_member_produces_exactly_one_terminal_record() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    // No tmux server backs this socket, so the target is unreachable and the
    // member resolves on the dwell rather than on a write that never happens.
    super::configure_short_unreachable_dwell();
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

/// Polls for the first inscription line for `event`. The terminal record is
/// written by the delivery worker task rather than on the request path, so it is
/// not present when the send response returns.
fn await_inscription(path: &std::path::Path, event: &str) -> String {
    await_inscription_within(path, event, std::time::Duration::from_secs(5))
}

/// `await_inscription` with an explicit bound, for the one scenario whose
/// completion is gated on a transport's own wait rather than on the relay.
fn await_inscription_within(
    path: &std::path::Path,
    event: &str,
    bound: std::time::Duration,
) -> String {
    let deadline = std::time::Instant::now() + bound;
    loop {
        if let Some(line) = read_inscriptions(path, event).into_iter().next() {
            return line;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {event} inscription within {bound:?}"
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

/// A target unreachable past the dwell resolves `not_submitted`, not `failed`.
///
/// The member was never authorized and never handed to a transport, so nothing
/// could have reached the target. That is positive evidence of non-delivery,
/// which is what `not_submitted` asserts; `failed` would report a delivery
/// attempt that never happened. The distinction is the whole point of the guard's
/// evidence order, and reporting the weaker spelling would make an honest
/// non-delivery indistinguishable from a write that went wrong.
#[test]
fn a_sustained_unreachable_target_resolves_not_submitted() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    // No tmux server backs this socket, so the target is unreachable from the
    // first observation and stays that way.
    super::configure_short_unreachable_dwell();
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
    assert_eq!(
        payload.get("outcome").and_then(serde_json::Value::as_str),
        Some("not_submitted"),
        "an unreachable target's member resolves not_submitted: {completed}"
    );
    assert_eq!(
        payload
            .get("reason_code")
            .and_then(serde_json::Value::as_str),
        Some("delivery_target_unreachable"),
        "the unreachable reason code survives the terminal transition: {completed}"
    );
}

/// An unreachable target under the dwell is held, never authorized.
///
/// Both axes gate a delivery attempt, and readiness cannot stand in for health:
/// a transport that cannot reach its target has nothing useful to say about
/// whether that target is at a prompt. An earlier version read health, declined
/// to resolve under the dwell, and then let a successful readiness probe
/// authorize anyway — which is the two-axis rule holding in the spec and not in
/// the code.
///
/// Absence is the assertion here, so the teeth are in the dwell: it is long
/// enough that resolution cannot be what suppressed the record.
#[test]
fn an_unreachable_target_under_the_dwell_is_never_authorized() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    super::configure_long_unreachable_dwell();
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

    // Several worker poll ticks: long enough that an authorization would have
    // produced its terminal record by now, far short of the dwell.
    std::thread::sleep(std::time::Duration::from_millis(600));

    assert_eq!(
        count_inscriptions(&inscriptions, "relay.send.async.completed"),
        0,
        "a member held under the dwell produces no terminal record"
    );
}
