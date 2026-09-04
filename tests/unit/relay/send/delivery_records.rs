//! The audit trail a delivery leaves: batch and partition identity, their
//! ordering, and the one terminal record a member is allowed.

use agentmux::relay::RelayRequest;

use super::*;

/// A member's guard is keyed by its position in its target's mailbox, so the
/// terminal record can name the entry a delivery resolved under.
///
/// Without that identity on the wire the state model would be internal
/// bookkeeping no operator could correlate — a stuck target's outcome could not
/// be traced back to the entry that produced it.
///
/// The target is the UI session rather than a tmux one, and that is load-bearing.
/// A tmux target with no server behind it is unreachable, so its member resolves
/// before anything takes responsibility for it and never acquires a guard at all
/// — a correct outcome that simply is not the one under test. UI reports healthy
/// and ready unconditionally, so it reaches the guarded state this test is about.
#[test]
fn a_terminal_outcome_names_the_entry_it_resolved_under() {
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
            .get("entry_sequence")
            .and_then(serde_json::Value::as_u64)
            .is_some(),
        "a guarded member's terminal record carries its mailbox position: {completed}"
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
        "a UI transport peeks one entry, so its unit has one member: {declared}"
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

/// The enqueue reaches the log ahead of the partition, naming the position the
/// relay committed the member to before anything bound it.
///
/// Two claims, and the second is the one worth the test. Naming the enqueue
/// closes the antecedent of every per-member attribution downstream: the
/// partition says which members shared a *submission*, and only the enqueue says
/// which position each of them occupied — a reader with just the partition cannot
/// tell a run an executor peeked together from entries that merely happened to be
/// adjacent.
///
/// The ordering is the contract, not an artifact of how the code happens to run
/// today. Admission into the mailbox is the linearization point, so a partition
/// recorded ahead of it would mean a member had been bound to a unit — the point
/// past which non-delivery can no longer be proven — before the relay had given
/// it a position to be delivered from at all. Asserting the order here is what
/// makes that falsifiable from outside the relay.
///
/// A singleton, and for a sharper reason than the partition test's: what an
/// executor may peek together is bounded by its declared peek dimensions, and a
/// UI transport declares one envelope. Multi-member runs wait on a transport that
/// coalesces, so there is no load this test could apply that would produce one.
#[test]
fn an_enqueued_entry_precedes_the_partition_that_binds_it() {
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
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    let enqueued = await_inscription_within(
        &inscriptions,
        "relay.delivery.mailbox.enqueued",
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
        serde_json::from_str(enqueued.as_str()).expect("enqueue inscription is json");
    let payload = record
        .get("details")
        .expect("the enqueue carries a details object");

    assert!(
        payload
            .get("sequence")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|sequence| sequence > 0),
        "an enqueue names the position it issued: {enqueued}"
    );
    let member = payload
        .get("message_id")
        .and_then(serde_json::Value::as_str)
        .expect("the enqueued member is named");
    assert!(!member.is_empty(), "the enqueued member is named");

    // The same member, and the enqueue first. Read the whole log rather than
    // relying on the two awaits above, because each returns as soon as its own
    // event appears and neither says which was written first.
    let log = std::fs::read_to_string(&inscriptions).expect("inscriptions readable");
    let position = |event: &str| {
        log.lines()
            .position(|line| line.contains(format!("\"event\":\"{event}\"").as_str()))
            .unwrap_or_else(|| panic!("no {event} in the log: {log}"))
    };
    assert!(
        position("relay.delivery.mailbox.enqueued") < position("relay.delivery.partition.declared"),
        "the entry is enqueued before its partition is declared: {log}"
    );
    let partition = log
        .lines()
        .find(|line| line.contains("\"event\":\"relay.delivery.partition.declared\""))
        .expect("the partition is in the log");
    assert!(
        partition.contains(member),
        "the partition covers the member the enqueue named: {partition}"
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
