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
/// termination primitive. On the shutdown path the cooperative step reaches the
/// agent by other means, so the fence still reaches a positive verdict with that
/// primitive reverted. Termination's teeth were only demonstrable under the
/// execution watchdog, which is no longer armed; if it is re-armed, restore the
/// watchdog-driven variant of this test with it.
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
    let child_pids = await_recorded_child_pids(&pid_path);

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
    assert!(
        verdict.contains("\"resolution\":\"forced\""),
        "the ACP reader parks in a blocking read, so no cooperative request can \
         reach it and the destructive step is always required: {verdict}"
    );

    for pid in child_pids.iter().copied() {
        await_process_gone(pid);
    }

    // Settle past the monitor's poll interval and a respawn's backoff, so a
    // replacement that was going to be installed would have appeared by now.
    std::thread::sleep(Duration::from_secs(2));
    let after = std::fs::read_to_string(&pid_path).unwrap_or_default();
    let started_total = after
        .lines()
        .filter(|line| line.trim().parse::<i32>().is_ok())
        .count();
    assert_eq!(
        started_total,
        child_pids.len(),
        "a fenced generation must admit no replacement agent; pid log: {after:?}"
    );
}

/// Every child pid the stub has recorded so far, in the order it started them.
fn recorded_child_pids(path: &Path) -> Vec<i32> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
        .collect()
}

/// Polls until the stub has recorded at least one child pid.
fn await_recorded_child_pids(path: &Path) -> Vec<i32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pids = recorded_child_pids(path);
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
/// Ignored by default because the respawn backoffs that get a bootstrap open at
/// the right moment cost about thirty seconds, which is too much to pay on every
/// commit. The pre-push hook runs it; see
/// `.auxiliary/configuration/pre-commit.yaml`.
#[test]
#[ignore = "~30s: holds a bootstrap open across a signal; run at pre-push"]
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

    // Both bundle members respawn, so a replacement agent for every one of them
    // has to be up before the signal; otherwise the fence could land before the
    // bootstrap this test is about has started.
    let pid_path = acp_child_pid_path(temporary.path());
    await_recorded_agents(&pid_path, 4);
    // The most recent agent is the one parked in `initialize`, holding this
    // respawn's bootstrap open. Earlier ones have already been killed and reaped
    // by the respawns that replaced them, so they say nothing about the fence.
    let hung_agent = *recorded_child_pids(&pid_path)
        .last()
        .expect("at least one agent recorded");

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
    let child_pids = await_recorded_child_pids(&pid_path);

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

    // The verdict claims cessation; this is the process it claimed it about.
    for pid in child_pids.iter().copied() {
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

/// Polls until the stub has recorded at least `count` agents.
fn await_recorded_agents(path: &Path, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let recorded = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.trim().parse::<i32>().is_ok())
            .count();
        if recorded >= count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "only {recorded} of {count} agents started within 20s"
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
