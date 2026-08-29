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

use super::helpers::*;

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

mod fence;
mod shutdown;
mod watchdog;
