//! An unreachable target: how a member resolves once the dwell elapses, and
//! what must not happen to it before then.

use agentmux::relay::RelayRequest;

use super::*;

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
    configure_short_unreachable_dwell();
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

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
    configure_long_unreachable_dwell();
    let config_root = write_bundle(&temporary, "party");
    let tmux_socket = temporary.path().join("tmux.sock");

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

    // Several worker poll ticks: long enough that an authorization would have
    // produced its terminal record by now, far short of the dwell.
    std::thread::sleep(std::time::Duration::from_millis(600));

    assert_eq!(
        count_inscriptions(&inscriptions, "relay.send.async.completed"),
        0,
        "a member held under the dwell produces no terminal record"
    );
}
