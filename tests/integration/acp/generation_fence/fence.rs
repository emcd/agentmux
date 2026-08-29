use super::*;

/// Forced termination must reach the ACP child, and cessation must be observed
/// on the reader that is actually running.
///
/// The ACP client is moved into the delivery task it drives, which empties the
/// transport's runtime slot for the whole steady state. Reading termination and
/// cessation off that slot therefore made step 3 address nothing and made the
/// reader observation vacuously true — a fence that could report a generation
/// stopped while its child was still alive and its reader still parked in
/// `read_line`.
///
/// **What this does not prove.** It does not discriminate ACP's forced
/// termination primitive, and it deliberately asserts nothing about which
/// resolution the fence reaches. This stub keeps reading its stdin while
/// ignoring the prompt, so the cooperative step — which drops the write channel
/// and closes that stdin — can end the agent on its own. Whether it does so
/// inside the first observation window is a race with the process exiting, so
/// the resolution is legitimately either `cooperative` or `forced`. An earlier
/// version asserted `forced` and was flaky for exactly that reason, while its
/// comment claimed the reader always parks somewhere no cooperative request can
/// reach — which is true of the *bootstrap* tests below, where the agent never
/// answers, and false here.
///
/// Termination's teeth live in the watchdog-driven variant below,
/// `an_executor_blocked_past_the_bound_is_fenced_and_its_member_resolved`, and
/// they are real: neutering ACP's `initiate_termination` fails that test and
/// leaves this one passing. The watchdog does arm — for a member still
/// unresolved past the bound, which is what a parked write produces — so the
/// note that once stood here asking a future reader to restore a watchdog-driven
/// variant has been overtaken by that test existing.
///
/// What this test holds down is the fate of the process. That is worth stating
/// narrowly: with every explicit destructive step neutered — ACP's
/// `initiate_termination`, its `bootstraps.initiate_termination`, and the
/// driver's two task aborts — the child still dies and this test still passes,
/// so the process fate it observes is carried by ownership rather than by the
/// fence's step 3. Which is not a defect; a `Drop` that kills and waits is a
/// sound way for a child to die. It does mean this test cannot be cited as
/// evidence that the destructive step reaches anything.
///
/// The second half asserts the invariant that lets the guard key drop its
/// generation component: a fenced generation admits no replacement. The dying
/// agent signals respawn-needed, and the driver-owned monitor would answer it by
/// bootstrapping a fresh one — a second live agent for a target the relay has
/// just declared stopped. Counting the agents the stub ever started is what
/// catches that, because the relay's own account of a generation cannot.
///
/// Driven by a real SIGTERM, because graceful shutdown is the only thing that
/// fences a generation today. The execution watchdog that used to drive this is
/// unarmed: anchored at authorization, it measured the agent's inference rather
/// than the relay's own execution, and fenced healthy targets mid-turn.
#[test]
fn a_fenced_acp_generation_leaves_no_surviving_child() {
    let temporary = GuardedTempDir::new();
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    // A turn that never comes back, so the member is still in flight when the
    // signal lands. Deliberately not a `sleep`-based stall: that helper process
    // would inherit the agent's stdout, so ending the agent would leave the pipe
    // open and no reader could ever observe EOF.
    let options = AcpStubOptions {
        never_respond_to_prompt: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        fence_observation_timeout_ms: 500,
        ..DeliveryConfiguration::default()
    });

    let result = send_result(dispatch_send(&config_root, &tmux_socket));
    assert_eq!(result.outcome, SendOutcome::Queued);

    let pid_path = acp_child_pid_path(temporary.path());
    let child_pids = await_recorded_child_pids(&pid_path, "bravo");

    // The real signal, not a test-only flag: this is the production trigger.
    // The guard restores the previous handlers and clears the flag on drop, so
    // it has to outlive every assertion below.
    let _signal_guard = agentmux::runtime::signals::install_shutdown_signal_handlers()
        .expect("install shutdown signal handlers");
    let self_pid = i32::try_from(std::process::id()).expect("pid fits i32");
    assert_eq!(
        unsafe { libc::kill(self_pid, libc::SIGTERM) },
        0,
        "failed to signal this process"
    );

    // Scoped to the target under test. The bundle holds two ACP members, and
    // the other one is idle — its generation ceases trivially, so an unscoped
    // lookup reads a positive verdict that proves nothing about this one.
    let verdict = await_inscription(
        &inscriptions,
        "relay.delivery.fence.verdict",
        "\"target_session\":\"bravo\"",
    );
    assert!(
        verdict.contains("\"verdict\":\"positive\""),
        "the fence must establish cessation rather than fail stop: {verdict}"
    );

    for pid in child_pids.iter().copied() {
        await_process_gone(pid);
    }

    // Settle past the monitor's poll interval and a respawn's backoff, so a
    // replacement that was going to be installed would have appeared by now.
    std::thread::sleep(Duration::from_secs(2));
    let after = recorded_child_pids(&pid_path, "bravo");
    assert_eq!(
        after.len(),
        child_pids.len(),
        "a fenced generation must admit no replacement agent; recorded: {after:?}"
    );
}

/// A fence must reach a *respawn's* bootstrap too, not only the initial one.
///
/// The two arrive by different routes. The initial bootstrap is started by the
/// worker; a respawn's is started by the driver-owned monitor, off the worker
/// loop, at a moment nothing coordinates with the fence. The monitor drives it on
/// a blocking pool, and `tokio`'s abort cancels only the task awaiting that
/// closure — never the closure itself — so the async wrapper says nothing about
/// whether the executor stopped, and that executor is the one holding an agent
/// child. A fence landing mid-respawn once reported *positive* within a second
/// while a live agent was being brought up behind it.
///
/// Counting the bootstrap fixed the false positive but only bought an honest
/// negative; ending it is what makes the verdict positive on its merits. Both
/// halves are asserted: revert the registry and this reports positive too early,
/// revert the bootstrap arm of step 3 and it reports negative forever.
///
/// The scenario is built from a first agent that disconnects on its prompt — so
/// the monitor starts a respawn — and a second that blocks inside `initialize`,
/// holding that respawn's bootstrap open across the signal.
#[test]
fn a_fence_ends_a_respawn_bootstrap() {
    let temporary = GuardedTempDir::new();
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let options = AcpStubOptions {
        disconnect_on_prompt: Some("before_activity".to_string()),
        hang_initialize_from_agent: Some(1),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        fence_observation_timeout_ms: 500,
        ..DeliveryConfiguration::default()
    });

    let _ = send_result(dispatch_send(&config_root, &tmux_socket));

    // The target's *second* agent is the respawn this test is about: the first
    // disconnected on its prompt, and the replacement is the one that hangs in
    // `initialize`. Waiting on the target's own count rather than a bundle-wide
    // one is what makes this the respawn of `bravo` and not of whichever member
    // happened to reach two first.
    let pid_path = acp_child_pid_path(temporary.path());
    await_recorded_agents(&pid_path, "bravo", 2);
    // `bravo`'s most recent agent is the one parked in `initialize`, holding this
    // respawn's bootstrap open. Read from `bravo`'s own records: both members
    // respawn into the same file, so the last line overall belongs to whichever
    // of them happened to start last, which is not the target this asserts on.
    // Earlier agents were killed and reaped by the respawns that replaced them,
    // so they say nothing about the fence.
    let hung_agent = *recorded_child_pids(&pid_path, "bravo")
        .last()
        .expect("at least one agent recorded for the target");
    // A recorded pid only means the stub started; it is written before the
    // agent has read anything. The fifo is created immediately before the read
    // that blocks on it, so its existence is the first moment an agent is
    // actually parked in `initialize` with a bootstrap held open behind it.
    // Signalling on the pid alone left a window — wide enough to matter under
    // load — in which the fence could land before there was a hung bootstrap to
    // find, and then resolve cooperatively.
    await_hang_fifo(temporary.path());

    let _signal_guard = agentmux::runtime::signals::install_shutdown_signal_handlers()
        .expect("install shutdown signal handlers");
    let self_pid = i32::try_from(std::process::id()).expect("pid fits i32");
    assert_eq!(
        unsafe { libc::kill(self_pid, libc::SIGTERM) },
        0,
        "failed to signal this process"
    );

    // Scoped to the target under test: an unscoped lookup reads whichever
    // sibling's verdict landed first and proves nothing about this one.
    let verdict = await_inscription(
        &inscriptions,
        "relay.delivery.fence.verdict",
        "\"target_session\":\"bravo\"",
    );
    assert!(
        verdict.contains("\"verdict\":\"positive\""),
        "the fence must end the respawn's bootstrap rather than wait out an \
         operation timeout it does not control: {verdict}"
    );
    assert!(
        verdict.contains("\"resolution\":\"forced\""),
        "a bootstrap parked on an agent that will not answer is not reachable by \
         any cooperative request, so reaching this positive without escalating \
         would mean the bootstrap was not in flight at all: {verdict}"
    );

    // The verdict claims cessation; this is the process it claimed it about.
    await_process_gone(hung_agent);

    // A safety net, not a step: nothing should still be parked on the fifo once
    // the fence has ended these children, but a writer costs nothing and an
    // agent left blocked there would outlive the test.
    release_hung_initialize(temporary.path());
}

/// A fence must reach, and end, a hung *initial* bootstrap.
///
/// Two things are load-bearing here, and each fails differently.
///
/// The fence has to begin at all. The initial bootstrap used to run before the
/// delivery loop was ever entered — the worker awaited it — so an agent parked
/// in its `initialize` handshake left the worker with no shutdown gate. No fence
/// began, no verdict was emitted, and the relay's account of that target on the
/// way out was silence rather than a finding. Reverting that makes this test time
/// out waiting for an inscription nothing writes.
///
/// Then the forced step has to reach the child. A bootstrap is held where it is
/// by its agent failing to answer, so signalling that child is the only thing
/// that ends it; the relay does not control the operation timeout that would
/// otherwise expire. Reverting the bootstrap arm of step 3 turns this negative —
/// still honest, but honest about a generation the relay could not stop.
///
/// So the assertion is a *forced positive* against a live process: the verdict
/// says cessation was established, and the pid says the agent it was established
/// about is gone.

#[test]
fn a_fence_ends_a_hung_initial_bootstrap() {
    let temporary = GuardedTempDir::new();
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    // From the very first agent: the target this asserts on has never had a
    // runtime, so the bootstrap in flight is its initial one.
    let options = AcpStubOptions {
        hang_initialize_from_agent: Some(0),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        fence_observation_timeout_ms: 500,
        ..DeliveryConfiguration::default()
    });

    // Startup waits on a target whose agent never answers, so it runs on its own
    // thread; this test is about what the worker does while that wait is
    // happening.
    let (dispatch_done, dispatch_finished) = std::sync::mpsc::channel();
    let dispatch_roots = config_root.clone();
    let dispatch_socket = tmux_socket.clone();
    let dispatcher = std::thread::spawn(move || {
        let _ = dispatch_send_result(&dispatch_roots, &dispatch_socket);
        let _ = dispatch_done.send(());
    });

    // The agent is up and parked in `initialize`: `alpha` is the first member
    // the bundle starts, so this pid is its initial bootstrap's child, and
    // nothing but the fence will end it.
    let pid_path = acp_child_pid_path(temporary.path());
    await_recorded_child_pids(&pid_path, "alpha");

    let _signal_guard = agentmux::runtime::signals::install_shutdown_signal_handlers()
        .expect("install shutdown signal handlers");
    let self_pid = i32::try_from(std::process::id()).expect("pid fits i32");
    assert_eq!(
        unsafe { libc::kill(self_pid, libc::SIGTERM) },
        0,
        "failed to signal this process"
    );

    let verdict = await_inscription(
        &inscriptions,
        "relay.delivery.fence.verdict",
        "\"target_session\":\"alpha\"",
    );
    assert!(
        verdict.contains("\"verdict\":\"positive\""),
        "the fence must end the bootstrap rather than wait out an operation \
         timeout it does not control: {verdict}"
    );
    assert!(
        verdict.contains("\"resolution\":\"forced\""),
        "a bootstrap parked on an agent that will not answer is not reachable by \
         any cooperative request: {verdict}"
    );

    // Read *after* the verdict, not snapshotted before the signal. A child
    // spawned between the signal and the verdict is precisely the one a
    // one-shot traversal of the bootstrap registry would have missed: it
    // publishes into a record the forced step has already walked past, so a
    // before-snapshot cannot name it and the assertion would pass over the leak.
    // Settle past a respawn's poll interval first, so a replacement that was
    // going to appear has.
    std::thread::sleep(Duration::from_millis(500));
    let recorded = recorded_child_pids(&pid_path, "alpha");
    assert!(
        !recorded.is_empty(),
        "the target must have started at least one agent"
    );
    for pid in recorded {
        await_process_gone(pid);
    }

    // Release anything still parked in `initialize` so the startup thread
    // returns rather than leaving children orphaned on the fifo.
    await_dispatch_thread(&dispatch_finished, temporary.path());
    dispatcher.join().expect("startup thread");
}

/// The execution watchdog against a real executor parked mid-write.
///
/// This is the state the watchdog exists for and the only one that reaches it:
/// the agent is alive, the target is reachable and healthy, and the relay is
/// blocked inside its own framed write because the pipe buffer filled and the
/// agent stopped draining it. Neither of the two delivery-model conditions —
/// relay shutting down, transport unhealthy — fires, so without a bound the
/// member stays `Authorized` forever, holding quota and blocking the target FIFO.
///
/// It also restores the coverage the shutdown-driven fence test above records as
/// owed: termination's teeth against a *steady-state* generation. There the
/// cooperative step can end the agent on its own, so the resolution is
/// legitimately a race. Here it cannot — a shell blocked reading a fifo observes
/// no flag and answers no channel drop, so only killing the child can free the
/// relay's parked write, and only that makes cessation observable.
///
/// **What this does not discriminate.** Whether the member terminalized at the
/// bound or at the verdict. Step 3 kills the child, which fails the parked write,
/// which resolves the member through the transport's own evidence — legitimately
/// inside the second observation window, since evidence stays admissible right
/// through it. So the ordering between the terminal outcome and the verdict is
/// genuinely either way, and asserting one would be asserting a race. The
/// single-cut rule is pinned by the protocol tests in `unit/delivery_fence.rs`
/// and by there being one call site; what is pinned here is everything else.
#[test]
fn an_executor_blocked_past_the_bound_is_fenced_and_its_member_resolved() {
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
        submission_timeout_ms: 500,
        fence_observation_timeout_ms: 500,
        ..DeliveryConfiguration::default()
    });

    // Comfortably past a pipe's 64 KiB buffer and well under the 256 KiB
    // handover maximum, so admission accepts it and the write cannot complete.
    let result = send_result(
        dispatch_sized_send_result(&config_root, &tmux_socket, 150_000)
            .expect("relay request should parse"),
    );
    assert_eq!(result.outcome, SendOutcome::Queued);

    let pid_path = acp_child_pid_path(temporary.path());
    let first_agents = await_recorded_child_pids(&pid_path, "bravo");

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
        verdict.contains("\"target_session\":\"bravo\""),
        "the watchdog fence must be scoped to the wedged target: {verdict}"
    );
    assert!(
        verdict.contains("\"verdict\":\"positive\""),
        "killing the child frees the parked write, so cessation must be observed: {verdict}"
    );
    // Not incidental, and the reason this scenario can hold termination to
    // account where the shutdown-driven test above cannot. A shell blocked
    // reading a fifo observes no cooperative flag and answers no dropped
    // channel, so the first window must elapse unsatisfied and step 3 must be
    // what frees the write. A `cooperative` resolution here would mean the agent
    // ended for some reason of the harness's own, and every assertion below it
    // would be measuring that instead.
    assert!(
        verdict.contains("\"resolution\":\"forced\""),
        "only forced termination can free an executor parked in a full pipe: {verdict}"
    );

    for pid in first_agents.iter().copied() {
        await_process_gone(pid);
    }

    // `submission_unknown`, and specifically not a failure spelling. Bytes had
    // already gone into the pipe when the fence landed, so non-delivery is not
    // provable — and the bound asserts nothing about the target's health either
    // way, which is what keeps it distinct from the timers this change retires.
    let completed = await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        "\"target_session\":\"bravo\"",
    );
    assert!(
        completed.contains("\"outcome\":\"submission_unknown\""),
        "a member bound to a unit and cut short must resolve unknown: {completed}"
    );

    // A positive verdict releases replacement, so the worker builds a fresh
    // generation in place and its bootstrap starts a second agent. This is the
    // half a negative verdict would fail-stop, and the opposite of what the
    // shutdown-driven fence above asserts.
    await_recorded_agents(&pid_path, "bravo", first_agents.len() + 1);
}

// The watchdog must not arm on an agent that is merely slow to answer.
//
// This is the failure the whole arming precondition guards against, and the
// reason the bound sat unarmed until submission evidence moved to the write
// boundary. The stub takes the prompt and never responds, so the turn runs
// indefinitely — but the member resolved `delivered` at the framed write, so
// nothing is in flight and there is nothing for a bound anchored at
// authorization to measure.
//
// Its teeth are in the precondition it inverts: move ACP's member resolution
// back to the end of the turn and the member stays in flight, the watchdog arms
// inside half a second, and a healthy agent gets fenced mid-inference.
