//! The undelivered-queue report: what counts as backlogged, what a warning
//! names, and how often each is emitted.

use agentmux::relay::RelayRequest;

use super::*;

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
    configure_long_unreachable_dwell();
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
    configure_long_unreachable_dwell();
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

/// A warning counts what is *waiting* and ages from the oldest of those, not
/// from the reservation ledger.
///
/// The two disagree on exactly one shape: a target holding an `Authorized`
/// member and `Pending` members at the same time. `per_target` is incremented at
/// admission and decremented at release, so it counts the member being written
/// to right now; the waiting tally does not. Every other test in this cluster
/// leaves them equal, which is why the fix they cover passes with either reading.
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
