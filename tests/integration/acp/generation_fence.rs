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
/// The load-bearing assertion is the dead child. A positive verdict alone would
/// not discriminate: the failure mode produces one too, by observing an executor
/// set it never had.
///
/// The second half asserts the invariant that lets the guard key drop its
/// generation component: a fenced generation admits no replacement. The dying
/// agent signals respawn-needed, and the driver-owned monitor would answer it by
/// bootstrapping a fresh one — a second live agent for a target the relay has
/// just declared stopped. Counting the agents the stub ever started is what
/// catches that, because the relay's own account of a generation cannot.
#[test]
fn a_fenced_acp_generation_leaves_no_surviving_child() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    // A turn that never comes back, so the watchdog elapses with the member
    // still unresolved. Deliberately not a `sleep`-based delay: that helper
    // process would inherit the agent's stdout, so killing the agent would
    // leave the pipe open and the reader could never observe EOF.
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

    let pid_path = acp_child_pid_path(temporary.path());
    let child_pids = await_recorded_child_pids(&pid_path);

    let verdict = await_inscription(&inscriptions, "relay.delivery.fence.verdict");
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

/// Polls for the first inscription line naming `event`.
fn await_inscription(path: &Path, event: &str) -> String {
    let needle = format!("\"event\":\"{event}\"");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(line) = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .find(|line| line.contains(&needle))
        {
            return line.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "no {event} inscription within 15s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
