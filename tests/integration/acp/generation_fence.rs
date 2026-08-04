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

/// Polls until the stub has recorded at least one child pid.
fn await_recorded_child_pids(path: &Path) -> Vec<i32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pids: Vec<i32> = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse::<i32>().ok())
            .collect();
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

/// A fence must not report cessation while a bootstrap is still running.
///
/// The ACP respawn monitor drives its bootstrap on a blocking pool, and
/// `tokio`'s abort cancels only the task awaiting that closure — never the
/// closure itself. Observing the async wrapper therefore says nothing about
/// whether the executor stopped, and that executor is the one that spawns and
/// owns an agent child. Before this was counted, a fence landing mid-respawn
/// reported *positive* within a second while a live agent was being brought up
/// behind it.
///
/// The negative verdict is the whole assertion. It is the honest answer, and it
/// is fail-stop: the relay declines to claim a generation stopped when it cannot
/// see one of its executors. Reverting the in-flight count turns this positive.
///
/// The scenario is built from a first agent that disconnects on its prompt — so
/// the monitor starts a respawn — and a second that blocks inside `initialize`,
/// holding that respawn's bootstrap open across the signal.
#[test]
fn a_fence_does_not_report_cessation_while_a_bootstrap_runs() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let options = AcpStubOptions {
        disconnect_on_prompt: Some("before_activity".to_string()),
        hang_initialize_on_respawn: true,
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
        verdict.contains("\"verdict\":\"negative\""),
        "a generation with a bootstrap still running has not been observed to \
         cease: {verdict}"
    );

    // Release the hung agent so the blocking bootstrap returns rather than
    // sitting out its full operation timeout while the process tears down.
    release_hung_initialize(temporary.path());
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

/// Opens the hang fifo for writing, which is what unblocks an agent parked in
/// `initialize`.
fn release_hung_initialize(root: &Path) {
    let fifo = root.join("acp_hang_initialize.fifo");
    if fifo.exists() {
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
    }
}
