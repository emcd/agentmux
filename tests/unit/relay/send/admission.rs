//! Admission: what a send is refused for before any member is queued, and the
//! boundary cases that keep each refusal from widening.

use agentmux::relay::{RelayRequest, RelayResponse, SendOutcome};

use super::*;

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
