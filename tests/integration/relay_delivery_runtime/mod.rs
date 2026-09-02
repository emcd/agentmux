//! Relay runtime tests driven against a real relay child process and a fake
//! tmux, grouped by the concern each one pins:
//! - [`startup`]: bring-up — create retries, and the environment the owned
//!   tmux session is created with.
//! - [`shutdown`]: what a termination signal must accomplish before the
//!   process exits, and how in-flight members resolve.
//! - [`connections`]: connection admission, the worker-queue limit, and
//!   reaping idle pre-hello connections.
//! - [`delivery`]: what reaches the pane — the paste sequence, envelope
//!   metadata, canonical addressing, and what admission refuses outright.
//! - [`mailbox`]: what the relay's per-target mailbox holds while the push
//!   path is still the only thing delivering out of it.
//! - [`raww`]: raww's literal-text delivery and its submit behaviour.
//!
//! Helpers shared across more than one of those clusters live in this hub:
//! the paste-line classifiers and buffer readers are used by [`delivery`],
//! [`mailbox`] and [`raww`], and the graceful-shutdown helper by nearly every
//! module.
//!
//! One helper deliberately stays with its only caller rather than moving
//! here — `RELAY_SIGNAL_EXIT_BUDGET` in [`shutdown`] — because the budget's
//! rationale is only legible beside the signal tests it bounds.
//!
//! Fixture writers and the relay spawners come from `crate::support` and are
//! imported per module.

use std::{
    fs,
    path::{Path, PathBuf},
};

use tokio::time::timeout;

use crate::support::process::HARNESS_CHILD_WAIT_DEFAULT;

mod connections;
mod delivery;
mod mailbox;
mod raww;
mod shutdown;
mod startup;

/// A bracketed (`-p`) paste of the message body into the target pane.
fn is_body_paste_line(line: &str) -> bool {
    line.contains(" paste-buffer ") && line.contains("-t %1") && line.contains(" -p ")
}

/// The unbracketed paste carrying the submit carriage return. Distinguished
/// from the body paste by the absence of the `-p` (bracketed) flag.
fn is_submit_paste_line(line: &str) -> bool {
    line.contains(" paste-buffer ") && line.contains("-t %1") && !line.contains(" -p ")
}

/// Every pane envelope the fake tmux has been asked to paste, in no particular
/// order. Each paste writes its buffer beside the tmux log, so the directory
/// holding the log is the whole record of what reached the panes.
fn read_all_paste_buffers(directory: &Path) -> Vec<String> {
    let mut contents = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return contents;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        if file_name
            .to_string_lossy()
            .starts_with("fake-tmux.log.buffer.")
            && let Ok(content) = fs::read_to_string(entry.path())
        {
            contents.push(content);
        }
    }
    contents
}

fn read_paste_buffer_content(log_file: &Path, paste_line: &str) -> String {
    let mut tokens = paste_line.split_whitespace();
    let buffer_name = tokens
        .by_ref()
        .skip_while(|token| *token != "-b")
        .nth(1)
        .expect("paste-buffer command should include -b NAME");
    let buffer_path = PathBuf::from(format!("{}.buffer.{buffer_name}", log_file.display()));
    fs::read_to_string(&buffer_path)
        .unwrap_or_else(|error| panic!("read paste buffer file {}: {error}", buffer_path.display()))
}

/// Graceful shutdown helper used by normal-path cleanup — SIGTERM + bounded
/// wait so the relay's generation fence in `src/relay/README.md` (fence.rs
/// five-step protocol) can reap its ACP-worker grandchildren, rather than
/// bypassing it with SIGKILL (which would orphan those grandchildren until
/// the Drop/escalation reap). The graceful path reduces that risk; a hard
/// timeout still escalates to SIGKILL (see Err arm below).
///
/// Uses `HARNESS_CHILD_WAIT_DEFAULT` (10s) rather than a tighter bound so
/// the helper is not tighter than the relay's own 5000ms shutdown-work
/// deadline plus nested drain/fence budgets (src/relay/README.md).
async fn shutdown_relay_gracefully(child: &mut crate::support::relay_delivery::RelayChildGuard) {
    let pid = child.id().expect("relay pid");
    let pid = i32::try_from(pid).expect("relay pid fits i32");
    // SAFETY: `child` is the relay process this test spawned; `kill` with
    // SIGTERM is the same graceful signal systemd delivers and the relay's
    // SIGTERM/SIGINT handlers initiate the fenced shutdown.
    let kill_result = unsafe { libc::kill(pid, libc::SIGTERM) };
    assert_eq!(kill_result, 0, "failed to send SIGTERM");
    let wait_result = timeout(HARNESS_CHILD_WAIT_DEFAULT, child.wait()).await;
    match wait_result {
        Ok(result) => {
            let status = result.expect("wait relay");
            assert!(
                status.success(),
                "relay should exit cleanly after SIGTERM, status={status}"
            );
        }
        Err(_) => {
            // Escalation only after graceful window — see fallback comments above.
            child.start_kill().expect("kill relay after timeout");
            panic!("timed out waiting for relay to exit after SIGTERM");
        }
    }
}
