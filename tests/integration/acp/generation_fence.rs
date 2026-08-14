//! The generation fence against a live ACP child.
//!
//! The other fence coverage drives a controllable generation, which pins the
//! protocol's ordering and bounds but says nothing about whether a production
//! adapter's steps reach anything. This one does the opposite: it asserts on the
//! fate of the real process, because that is the only thing that can tell a
//! termination primitive apart from a call that silently addresses a field which
//! is empty for the whole steady state.

use std::path::Path;
use std::time::{Duration, Instant};

use agentmux::relay::{DeliveryConfiguration, SendOutcome, configure_delivery};
use tempfile::TempDir;

use super::helpers::*;

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
/// Termination's teeth against a steady-state generation were only demonstrable
/// under the execution watchdog, which is no longer armed; if it is re-armed,
/// restore the watchdog-driven variant of this test with it. What this test
/// still holds down is the fate of the process, which is the part the relay's
/// own account of a generation cannot fake.
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
    let temporary = TempDir::new().expect("temporary");
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

/// Polls until an agent has reached the `initialize` hang, which it signals by
/// creating the fifo it is about to block on.
fn await_hang_fifo(root: &Path) {
    let fifo = root.join("acp_hang_initialize.fifo");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !fifo.exists() {
        assert!(
            Instant::now() < deadline,
            "no agent reached the initialize hang within 20s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Every child pid the stub has recorded for `target_session`, in the order it
/// started them.
///
/// Scoped by session because every member of the bundle appends to one file.
/// Taking a pid off the end of the whole file attributed whichever member
/// happened to start last to the target an assertion names — the same unscoped
/// reading that once let a fence assertion pass on an idle sibling's verdict.
fn recorded_child_pids(path: &Path, target_session: &str) -> Vec<i32> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(session, _)| *session == target_session)
        .filter_map(|(_, pid)| pid.trim().parse::<i32>().ok())
        .collect()
}

/// Polls until the stub has recorded at least one child pid for `target_session`.
fn await_recorded_child_pids(path: &Path, target_session: &str) -> Vec<i32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pids = recorded_child_pids(path, target_session);
        if !pids.is_empty() {
            return pids;
        }
        assert!(
            Instant::now() < deadline,
            "the ACP stub recorded no child pid within 10s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits for `pid` to leave the process table. `kill(pid, 0)` probes liveness
/// without delivering a signal, returning -1 (ESRCH) once the process is gone.
fn await_process_gone(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if unsafe { libc::kill(pid, 0) } == -1 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "ACP stub child {pid} survived the generation fence"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Polls for the first inscription line naming `event` and also containing
/// `scope`, so a line emitted for a different target cannot answer for this one.
fn await_inscription(path: &Path, event: &str, scope: &str) -> String {
    let needle = format!("\"event\":\"{event}\"");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(line) = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .find(|line| line.contains(&needle) && line.contains(scope))
        {
            return line.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "no {event} inscription for {scope} within 15s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
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
///
#[test]
fn a_fence_ends_a_respawn_bootstrap() {
    let temporary = TempDir::new().expect("temporary");
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
    let temporary = TempDir::new().expect("temporary");
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

/// Waits for the startup thread to finish, releasing hung agents as they appear.
///
/// One writer opens releases one reader, and the bundle brings its members up
/// one at a time, so this has to keep offering rather than release once.
fn await_dispatch_thread(finished: &std::sync::mpsc::Receiver<()>, root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        release_hung_initialize(root);
        match finished.recv_timeout(Duration::from_millis(100)) {
            Ok(()) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => assert!(
                Instant::now() < deadline,
                "the startup thread did not return within 30s"
            ),
        }
    }
}

/// Polls until the stub has recorded at least `count` agents for `target_session`.
fn await_recorded_agents(path: &Path, target_session: &str, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let recorded = recorded_child_pids(path, target_session).len();
        if recorded >= count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "only {recorded} of {count} {target_session} agents started within 20s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Offers one writer to the hang fifo, releasing one agent parked in
/// `initialize` so its handshake proceeds.
///
/// Non-blocking, because opening a fifo for writing with no reader waiting on
/// the other end blocks the opener — which for a caller that polls would be the
/// test thread. A line is written rather than the handle merely closed, so the
/// agent's `read` succeeds and it answers `initialize` instead of taking an EOF
/// and dying under `set -e`.
fn release_hung_initialize(root: &Path) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let fifo = root.join("acp_hang_initialize.fifo");
    if let Ok(mut writer) = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
    {
        let _ = writer.write_all(b"go\n");
    }
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
    let temporary = TempDir::new().expect("temporary");
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

/// The watchdog must not arm on an agent that is merely slow to answer.
///
/// This is the failure the whole arming precondition guards against, and the
/// reason the bound sat unarmed until submission evidence moved to the write
/// boundary. The stub takes the prompt and never responds, so the turn runs
/// indefinitely — but the member resolved `delivered` at the framed write, so
/// nothing is in flight and there is nothing for a bound anchored at
/// authorization to measure.
///
/// Its teeth are in the precondition it inverts: move ACP's member resolution
/// back to the end of the turn and the member stays in flight, the watchdog arms
/// inside half a second, and a healthy agent gets fenced mid-inference.
#[test]
fn a_long_agent_turn_never_arms_the_execution_watchdog() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let options = AcpStubOptions {
        never_respond_to_prompt: true,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 500,
        fence_observation_timeout_ms: 500,
        ..DeliveryConfiguration::default()
    });

    let result = send_result(dispatch_send(&config_root, &tmux_socket));
    assert_eq!(result.outcome, SendOutcome::Queued);

    // Resolved at the write, while the turn it belongs to is still running.
    let completed = await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        "\"target_session\":\"bravo\"",
    );
    assert!(
        completed.contains("\"outcome\":\"delivered\""),
        "the framed write is the delivery boundary: {completed}"
    );

    // Several multiples of the bound, with the turn still in flight the whole
    // time. An armed watchdog would have fenced this target by now.
    std::thread::sleep(Duration::from_millis(2_500));
    let log = std::fs::read_to_string(&inscriptions).unwrap_or_default();
    assert!(
        !log.contains("relay.delivery.watchdog.armed"),
        "an agent that is slow to answer must not arm the execution watchdog: {log}"
    );
    assert!(
        !log.contains("\"trigger\":\"submission_timeout\""),
        "no fence may be initiated against a healthy target mid-turn: {log}"
    );
}

/// Terminal resolutions recorded for one message. Exactly-once is a claim about
/// this count, not about the outcome, so it is counted rather than found.
fn completions_for(path: &Path, message_id: &str) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|record| {
            record["event"] == "relay.send.async.completed"
                && record["details"]["message_id"] == message_id
        })
        .count()
}

/// The reported outcome for one message, once it has resolved.
fn outcome_for(path: &Path, message_id: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| {
            record["event"] == "relay.send.async.completed"
                && record["details"]["message_id"] == message_id
        })
        .and_then(|record| record["details"]["outcome"].as_str().map(str::to_string))
}

/// Polls until one message has a terminal resolution, then returns its outcome.
fn await_outcome(path: &Path, message_id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(outcome) = outcome_for(path, message_id) {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "message {message_id} never resolved within 15s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

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
    let temporary = TempDir::new().expect("temporary");
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
    let temporary = TempDir::new().expect("temporary");
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
