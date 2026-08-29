use super::*;

/// Graceful shutdown resolves a member parked inside its write and a member
/// still held behind it, each exactly once, and does not confuse the two.
///
/// The two states resolve through different code and owe the sender different
/// answers. A member the relay authorized and handed to a transport has an
/// evidence order to consult, and shutdown is only the trigger that makes it
/// read it; a member never authorized has no evidence at all, so shutdown can
/// say `dropped_on_shutdown` as a positive fact rather than inferring anything.
/// Collapsing them would let a member that may have reached its target be
/// reported as definitely dropped.
///
/// The parked member is produced the only way it can be: an agent that stops
/// draining its stdin after `session/new`, behind a body larger than the pipe
/// buffer, which leaves the relay's own executor blocked inside its framed
/// write. The held member is then simply the next send, because the transport
/// marks itself Busy on accept and the relay holds the following batch rather
/// than queueing it behind the turn. The submission timeout is set far out so
/// the execution watchdog cannot fence this generation first — shutdown is the
/// trigger under test, and a watchdog verdict would resolve the parked member
/// through a different cut entirely.
///
/// The held member is resolved from the worker's held slot rather than from its
/// receiver, which the teeth check isolates: removing only the held-slot branch
/// strands it for the full 15s budget, so the receiver drain behind it never saw
/// it. The parked member resolves through its own evidence path, and which
/// spelling that path reports is deliberately not asserted: the fence's forced
/// step frees the write, so the executor's own evidence is legitimately what
/// resolves it and the answer moves with that timing.
///
/// **What it does not cover, and cannot.** The other half of "mixed" — an
/// authorized member still unresolved when the post-fence guard drain runs, so
/// that the guard is what terminalizes it. A probe on that drain reports zero
/// members every time: forced termination frees the parked write, so the
/// collector always resolves the member first. Making the guard drain non-empty
/// needs an executor that survives forced termination, which is the same missing
/// injectable-transport seam that blocks the fail-stop path, and no real
/// transport provides one. The `assert_ne!` below is therefore weaker than it
/// looks — it guards against a collector-resolved member being reported as
/// dropped, not against the guard path being confused with the queue-drop path,
/// because this fixture never reaches the guard path at all. The completion
/// counts are tripwires for the same reason recorded on the replacement test
/// below: a probe records one `terminalize` attempt per message, so nothing here
/// contests the write-once transition.
#[test]
fn graceful_shutdown_resolves_a_parked_member_and_a_held_one_exactly_once() {
    let temporary = GuardedTempDir::new();
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let options = AcpStubOptions {
        stop_reading_stdin_after_new: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 60_000,
        fence_observation_timeout_ms: 500,
        ..DeliveryConfiguration::default()
    });

    let parked = send_result(
        dispatch_sized_send_result(&config_root, &tmux_socket, 150_000)
            .expect("relay request should parse"),
    );
    assert_eq!(parked.outcome, SendOutcome::Queued);

    // Past authorization and inside the write. Sending the second member before
    // this point would race the first into the same batch, and the test would be
    // about batch formation rather than about shutdown.
    await_inscription(
        &inscriptions,
        "relay.delivery.partition.declared",
        parked.message_id.as_str(),
    );

    let held = send_result(
        dispatch_send_without_startup_result(&config_root, &tmux_socket)
            .expect("relay request should parse"),
    );
    assert_eq!(held.outcome, SendOutcome::Queued);

    let _signal_guard = agentmux::runtime::signals::install_shutdown_signal_handlers()
        .expect("install shutdown signal handlers");
    let self_pid = i32::try_from(std::process::id()).expect("pid fits i32");
    assert_eq!(
        unsafe { libc::kill(self_pid, libc::SIGTERM) },
        0,
        "failed to signal this process"
    );

    let parked_outcome = await_outcome(&inscriptions, parked.message_id.as_str());
    let held_outcome = await_outcome(&inscriptions, held.message_id.as_str());

    assert_eq!(
        held_outcome, "dropped_on_shutdown",
        "a member shutdown found unauthorized owes the sender that positive fact"
    );
    // The discriminating assertion. This member reached a transport, so its
    // outcome has to come from the evidence order rather than from the queue-drop
    // path — whatever that order concludes, it is not this spelling.
    assert_ne!(
        parked_outcome, "dropped_on_shutdown",
        "a member already handed to a transport must not be reported as dropped"
    );

    assert_eq!(
        completions_for(&inscriptions, parked.message_id.as_str()),
        1,
        "the parked member resolves exactly once"
    );
    assert_eq!(
        completions_for(&inscriptions, held.message_id.as_str()),
        1,
        "the held member resolves exactly once"
    );
}

/// A generation replaced under a member in flight resolves that member exactly
/// once, returns its quota, and lets the target's FIFO make progress again.
///
/// Replacement is the dangerous direction for exactly-once. The old generation's
/// executor is still holding a write when the fence lands, and the new one is a
/// fresh transport for the same target — so a member could plausibly be resolved
/// by the old collector and again by whatever cleans up the replacement, or be
/// re-invoked against the new generation as if it had never been submitted.
/// Neither may happen: the member was authorized once and owes exactly one
/// answer.
///
/// The quota probe is what makes "returned" a fact rather than an inference. A
/// per-target maximum of one means the parked member owns the only slot, so the
/// send attempted beside it must be refused; if that refusal did not happen the
/// slot was never held and the release proves nothing. The send after the
/// replacement then has to be accepted *and* delivered, because an accepted send
/// that never arrives would look identical to a released slot on a target whose
/// FIFO had stopped moving.
///
/// The last send is deliberately small. The parked member is parked because 150
/// KiB cannot fit in the pipe buffer of an agent that has stopped reading; a
/// short body fits, so its framed write completes against the replacement agent
/// even though that agent is no more attentive than the first. What is being
/// shown is that the relay's path is clear, not that the new agent is reading.
///
/// **The completion counts are tripwires, not proofs.** A probe on `terminalize`
/// records exactly one attempt per message here, so nothing contests the
/// write-once transition and the counts cannot fail as written. They are kept
/// because a future change that introduced a second resolver would trip them,
/// which is worth the two lines — but the uniqueness this arc turns on is not
/// what they establish. Contesting the gate needs two resolvers racing for one
/// member, which no fixture in this arc currently constructs.

#[test]
fn a_generation_replaced_under_an_in_flight_member_resolves_it_once_and_frees_the_target() {
    let temporary = GuardedTempDir::new();
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let options = AcpStubOptions {
        stop_reading_stdin_after_new: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    // Far enough out that the quota probe below lands while the member is still
    // parked, and the watchdog is still what ends it.
    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 2_000,
        fence_observation_timeout_ms: 500,
        queued_envelopes_per_target_max: 1,
        ..DeliveryConfiguration::default()
    });

    let parked = send_result(
        dispatch_sized_send_result(&config_root, &tmux_socket, 150_000)
            .expect("relay request should parse"),
    );
    assert_eq!(parked.outcome, SendOutcome::Queued);

    let pid_path = acp_child_pid_path(temporary.path());
    let first_agents = await_recorded_child_pids(&pid_path, "bravo");

    await_inscription(
        &inscriptions,
        "relay.delivery.partition.declared",
        parked.message_id.as_str(),
    );

    // The slot is genuinely held while the member is in flight.
    let refused = dispatch_send_without_startup_result(&config_root, &tmux_socket)
        .expect_err("a per-target quota of one must refuse a second send");
    assert_eq!(refused.code, "runtime_delivery_queue_full");

    await_inscription(
        &inscriptions,
        "relay.delivery.watchdog.armed",
        "\"target_session\":\"bravo\"",
    );
    let verdict = await_inscription(
        &inscriptions,
        "relay.delivery.fence.verdict",
        "\"trigger\":\"submission_timeout\"",
    );
    assert!(
        verdict.contains("\"verdict\":\"positive\""),
        "killing the child frees the parked write, so cessation must be observed: {verdict}"
    );

    // A positive verdict releases replacement: the worker builds a fresh
    // generation in place and its bootstrap starts a second agent.
    await_recorded_agents(&pid_path, "bravo", first_agents.len() + 1);

    let parked_outcome = await_outcome(&inscriptions, parked.message_id.as_str());
    assert_eq!(
        completions_for(&inscriptions, parked.message_id.as_str()),
        1,
        "a member in flight across a replacement resolves exactly once, got {parked_outcome}"
    );

    // Quota returned, and the target's FIFO moves again on the new generation.
    let next = send_result(
        dispatch_send_without_startup_result(&config_root, &tmux_socket)
            .expect("the replaced target must accept a send once its slot is released"),
    );
    assert_eq!(next.outcome, SendOutcome::Queued);
    assert_eq!(
        await_outcome(&inscriptions, next.message_id.as_str()),
        "delivered",
        "a send after replacement must reach the new generation"
    );
    assert_eq!(
        completions_for(&inscriptions, next.message_id.as_str()),
        1,
        "the follow-up member also resolves exactly once"
    );
}

/// A member still `Pending` when its target's generation is replaced is
/// delivered by the replacement, rather than resolved against the generation it
/// never reached.
///
/// This is the half of the respawn contract the replacement test above does not
/// reach. That test sends its follow-up *after* the replacement has been
/// observed, which shows the target accepts new work again; it says nothing
/// about work that was already queued when the old generation died. The two
/// states owe the sender opposite answers — an `Authorized` member resolves
/// through the evidence order because it may have had an effect, while a
/// `Pending` one was never handed to anything and is still owed its delivery.
///
/// The held member is produced by the transport being busy rather than by any
/// contrivance: the parked member's prompt is in flight, ACP is single-flight,
/// and the relay holds the next batch instead of parking it in a transport
/// queue behind that turn. So `held` sits `Pending` in the worker across the
/// watchdog fence, the verdict, and the new generation's bootstrap.
///
/// `delivered` is the discriminating outcome. Reaching it requires a framed
/// write to have succeeded, and the only agent able to accept one is the
/// replacement — the first stopped reading its stdin after `session/new`, which
/// is what parked the member ahead of it. A `Pending` member dropped at the
/// fence, or resolved against the old generation, cannot produce it.
///
/// The per-target quota is two so the second send is admitted rather than
/// refused; the replacement test holds it at one precisely to prove the
/// refusal, which is the opposite fixture.

#[test]
fn a_pending_member_survives_a_generation_replacement_and_the_new_one_delivers_it() {
    let temporary = GuardedTempDir::new();
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let options = AcpStubOptions {
        stop_reading_stdin_after_new: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 2_000,
        fence_observation_timeout_ms: 500,
        queued_envelopes_per_target_max: 2,
        ..DeliveryConfiguration::default()
    });

    let parked = send_result(
        dispatch_sized_send_result(&config_root, &tmux_socket, 150_000)
            .expect("relay request should parse"),
    );
    assert_eq!(parked.outcome, SendOutcome::Queued);

    let pid_path = acp_child_pid_path(temporary.path());
    let first_agents = await_recorded_child_pids(&pid_path, "bravo");

    // Past authorization and inside the write. Sending the second member before
    // this point would race it into the same batch, and it would be authorized
    // against the generation about to die rather than held behind it.
    await_inscription(
        &inscriptions,
        "relay.delivery.partition.declared",
        parked.message_id.as_str(),
    );

    let held = send_result(
        dispatch_send_without_startup_result(&config_root, &tmux_socket)
            .expect("a per-target quota of two must admit a second send"),
    );
    assert_eq!(held.outcome, SendOutcome::Queued);

    let verdict = await_inscription(
        &inscriptions,
        "relay.delivery.fence.verdict",
        "\"trigger\":\"submission_timeout\"",
    );
    assert!(
        verdict.contains("\"verdict\":\"positive\""),
        "killing the child frees the parked write, so cessation must be observed: {verdict}"
    );
    await_recorded_agents(&pid_path, "bravo", first_agents.len() + 1);

    assert_eq!(
        await_outcome(&inscriptions, held.message_id.as_str()),
        "delivered",
        "a member still Pending at the replacement is owed delivery by the new generation"
    );
    assert_eq!(
        completions_for(&inscriptions, held.message_id.as_str()),
        1,
        "the rescheduled member resolves exactly once"
    );
    assert_eq!(
        completions_for(&inscriptions, parked.message_id.as_str()),
        1,
        "the member that was in flight across the replacement also resolves exactly once"
    );
}
