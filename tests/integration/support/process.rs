use std::{
    io::{self, Read},
    process::{Child, Output},
    thread,
    time::{Duration, Instant},
};

use agentmux::configuration::BringUpContext;

/// Removes inherited bring-up context from a test child's environment.
///
/// Configuration load stamps that context onto every agent-spawning member, so
/// a developer running this suite from an Agentmux-launched coder carries a
/// real identity in the test process environment, and every child spawned from
/// it inherits that identity. A `host mcp` child inheriting one resolves
/// association against the developer's bundle rather than the fixture's, so a
/// test covering absent or discovered association resolves a bundle which does
/// not exist under its temporary configuration root. That outcome depends on
/// who runs the suite and from where, which is precisely what a test result
/// must not depend on.
///
/// Call this before applying any test-supplied entries, so a child observes
/// context only where a test sets it deliberately. Keyed off the crate's own
/// enumeration, so extending the context cannot silently desync the sanitizer
/// from what the loader stamps.
pub(crate) fn strip_bring_up_context(
    command: &mut tokio::process::Command,
) -> &mut tokio::process::Command {
    for name in BringUpContext::VARIABLE_NAMES {
        command.env_remove(name);
    }
    command
}

/// Default budget for waiting on a test-spawned child to exit. Sized
/// so that even on heavily loaded CI a stuck child fails the test
/// quickly rather than hanging the suite. On timeout, the child is
/// sent SIGKILL and the wait returns an io::Error; callers typically
/// surface that as a test panic.
pub(crate) const HARNESS_CHILD_WAIT_DEFAULT: Duration = Duration::from_secs(10);

/// Poll interval for `wait_with_output_bounded`. Small enough that
/// a healthy child exiting near the deadline is observed with
/// millisecond precision; large enough that the loop is not a busy
/// spin.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Wait for `child` to exit, bounded by `budget`. Returns the
/// captured `Output` on success. On timeout, sends SIGKILL to the
/// child, reaps it, and returns an `io::Error` with kind `TimedOut`.
///
/// Replaces direct `Child::wait_with_output` calls in the harness
/// so that a wedged or orphaned child cannot block the suite
/// forever. An unbounded `wait_with_output` against a child that
/// holds the harness pipe and never exits will hang the test
/// process indefinitely (the orphaned child inherits the pipe's
/// write end and never delivers EOF).
///
/// Two non-obvious requirements this helper satisfies, which a
/// naive `try_wait` + `wait_with_output` sequence does NOT:
///
/// 1. `drop(child.stdin.take())` runs first, so a child that reads
///    stdin (e.g. `agentmux send` reading a piped message) sees
///    EOF promptly and exits on its own. Holding stdin open across
///    the wait would deadlock the child on its read.
///
/// 2. stdout and stderr are drained concurrently with the wait.
///    Without concurrent drain, a chatty child whose output fills
///    the ~64KB pipe buffer blocks in `write()` waiting for a
///    reader; the wait cannot return because the child cannot
///    exit; the suite deadlocks until the budget fires. Direct
///    `Child::wait_with_output` avoids this by spawning its own
///    drainer threads internally; this helper reproduces that
///    behavior so the bounded-wait convention does not introduce a
///    new pipe-full deadlock class.
pub(crate) fn wait_with_output_bounded(mut child: Child, budget: Duration) -> io::Result<Output> {
    drop(child.stdin.take());
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_thread = stdout.map(spawn_drainer);
    let stderr_thread = stderr.map(spawn_drainer);
    let deadline = Instant::now() + budget;
    let wait_result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "child did not exit within {:?}; sent SIGKILL and reaped",
                            budget
                        ),
                    ));
                }
                thread::sleep(WAIT_POLL_INTERVAL);
            }
            Err(error) => break Err(error),
        }
    };
    let stdout_bytes = join_drainer(stdout_thread);
    let stderr_bytes = join_drainer(stderr_thread);
    wait_result.map(|status| Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

fn spawn_drainer<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    })
}

fn join_drainer(handle: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    //! Lock-in tests for the helper's two non-obvious requirements
    //! (stdin drop + concurrent pipe drain). Per the project's
    //! testing-practices convention these are inline because the
    //! helper is crate-private by design (test-harness internal) and
    //! no public interface exercises the same code path.

    use super::*;
    use std::process::{Command, Stdio};

    /// Lock-in for the concurrent-drain requirement: a chatty
    /// child whose output exceeds the ~64KB pipe buffer must
    /// complete within the budget rather than deadlocking on
    /// `write()`. Without concurrent drain, the helper polls
    /// `try_wait` while stdout/stderr pipes fill; the child's
    /// next `write()` blocks waiting for a reader that does not
    /// exist; the child cannot exit; the budget fires SIGKILL
    /// on an otherwise-healthy child.
    #[test]
    fn chatty_child_with_pipe_drain_completes_within_budget() {
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("yes A | head -c 200000")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn chatty child");
        let output = wait_with_output_bounded(child, Duration::from_secs(5)).expect("chatty wait");
        assert!(output.status.success(), "chatty child should succeed");
        assert!(
            output.stdout.len() >= 200_000,
            "captured stdout should include the full 200KB; got {} bytes",
            output.stdout.len()
        );
    }

    /// Sanity counterpart to the chatty-child test: a silent child
    /// that writes nothing also completes cleanly within the budget.
    #[test]
    fn silent_child_completes_within_budget() {
        let child = Command::new("true")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn silent child");
        let output = wait_with_output_bounded(child, Duration::from_secs(5)).expect("silent wait");
        assert!(output.status.success(), "true should succeed");
        assert!(output.stdout.is_empty(), "stdout should be empty");
        assert!(output.stderr.is_empty(), "stderr should be empty");
    }

    /// A child that does not exit on its own must still be reaped
    /// within the budget via SIGKILL. Without the bounded helper,
    /// this scenario hangs the test process forever -- the child
    /// never exits, and `wait_with_output` has no deadline.
    ///
    /// Uses `/bin/sleep 60` directly (not `sh -c "..."`) so SIGKILL
    /// targets the single sleep process; if the helper were invoked
    /// against a shell that forks (e.g. `sh -c "sleep 60"`), SIGKILL
    /// would only kill the shell, leaving the forked `sleep` holding
    /// the inherited pipe write-ends and the drainer threads stuck
    /// in `read_to_end` until the orphan exits. The intent of this
    /// test is the helper's kill+wait+drain behavior, not its
    /// process-group handling.
    #[test]
    fn hung_child_is_killed_after_budget_expiry() {
        let child = Command::new("/bin/sleep")
            .arg("60")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hung child");
        let started = Instant::now();
        let result = wait_with_output_bounded(child, Duration::from_millis(500));
        let elapsed = started.elapsed();
        assert!(
            result.is_err(),
            "expected TimedOut error for child that exceeds the budget"
        );
        assert_eq!(
            result.unwrap_err().kind(),
            io::ErrorKind::TimedOut,
            "error kind should be TimedOut"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "helper should not hang past the budget; elapsed={elapsed:?}"
        );
    }
}
