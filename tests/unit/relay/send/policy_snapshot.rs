//! When authorization policy is judged, and what a change to it may reach.

use agentmux::relay::RelayRequest;

use super::*;

/// A policy change governs the next request and nothing already admitted.
///
/// Both halves are one claim and neither stands without the other. That an
/// admitted entry survives a tightening is uninteresting on its own — a relay
/// that never read the file again would satisfy it — so the refusal below is
/// what makes the survival a statement about *when* policy is judged rather than
/// about whether it is. And a refusal on its own would say nothing about the
/// entry already in the mailbox, which is the half the requirement exists for:
/// an entry's fate is settled at admission, so nothing downstream of it may
/// consult policy again.
///
/// The tightening is total — `send` drops to `none` for every session — so
/// anything re-checking policy at any later point would find the queued entry
/// forbidden. It survives because nothing re-checks, not because the change was
/// too narrow to reach it.
///
/// `bravo@party` is a tmux target with no server behind its socket, so it is
/// unreachable from the first observation, and under a dwell longer than this
/// test its member is held rather than resolved. That is what keeps an entry
/// sitting in the mailbox across the policy change with no separate mechanism to
/// suspend delivery.
#[test]
fn a_policy_tightening_governs_the_next_send_and_not_the_one_already_queued() {
    use agentmux::relay::{UndeliveredReporting, report_undelivered_queue};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);
    configure_long_unreachable_dwell();
    let config_root = write_bundle(&temporary, "party");
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
    };

    send().expect("a send permitted by the policy in effect is admitted");
    // Long enough for the member to have reached the state it will hold, far
    // short of the dwell.
    std::thread::sleep(std::time::Duration::from_millis(600));

    forbid_every_send(&config_root);

    // The policy in effect at the time of a request is what that request is
    // judged against, which is what a fresh read per request means. Without this
    // the assertions below would hold equally against a relay that read the file
    // once at startup and never again.
    let refused = send().expect_err("a send forbidden by the policy now in effect is refused");
    assert_eq!(
        refused.code, "authorization_forbidden",
        "the tightened policy refuses the new send: {refused:?}"
    );

    // Several more worker poll ticks, so a re-check anywhere downstream of
    // admission has had every opportunity to run.
    std::thread::sleep(std::time::Duration::from_millis(600));

    // Still waiting, and still holding the reservation it was admitted with.
    // This is the "remains queued and deliverable" half: the aggregate counts
    // entries that are queued and unguarded, so an entry purged by the change
    // would be missing here and one blocked by it would have had to resolve to
    // get that way.
    report_undelivered_queue(UndeliveredReporting::default());
    let aggregates = read_inscriptions(&inscriptions, "relay.delivery.undelivered");
    // A pass with nothing waiting emits no aggregate at all, so an absent record
    // reads as a queue holding nothing rather than as a reading not taken. That
    // keeps the assertion below on the claim itself — the entry is still there —
    // instead of on whether a report was written.
    let waiting = aggregates
        .first()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|record| {
            record
                .get("details")?
                .get("undelivered_envelopes_total")?
                .as_u64()
        })
        .unwrap_or(0);
    assert_eq!(
        waiting, 1,
        "the entry admitted under the old policy is still waiting to be delivered: \
         {aggregates:?}"
    );

    // And it was not resolved on account of the change. A re-check that found it
    // forbidden could only report that as a terminal outcome, so the absence of
    // one is what says no re-check happened.
    let completions = read_inscriptions(&inscriptions, "relay.send.async.completed");
    assert!(
        completions.is_empty(),
        "an entry admitted under the old policy is neither purged nor re-authorized: \
         {completions:?}"
    );

    // The refusal queued nothing, so the single entry above is the first send's
    // and the second left no residue behind the refusal it returned.
    assert_eq!(
        count_bravo_queued_inscriptions(&inscriptions),
        1,
        "a refused request admits nothing"
    );
}

/// Rewrites the bundle's policies so no session may send at all.
///
/// A total tightening rather than a targeted one: the point is that no reachable
/// re-check would find the queued entry permitted, so the survival cannot be
/// explained by the change having missed it.
fn forbid_every_send(roots: &agentmux::configuration::ConfigurationRoots) {
    std::fs::write(
        roots.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "none"
"#,
    )
    .expect("rewrite policies file");
}
