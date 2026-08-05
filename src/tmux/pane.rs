use std::{
    cell::RefCell,
    ffi::OsStr,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

const PASTE_BUFFER_NAME_PREFIX: &str = "agentmux-relay";
const LOOK_LINES_MAX: usize = 1000;

static PASTE_BUFFER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// How often a reap re-checks a still-running invocation.
const INVOCATION_REAP_POLL_MS: u64 = 10;

/// The `tmux` client invocation a generation's executor currently owns, and
/// whether that generation has been told to end.
///
/// The [`Child`] itself, not its pid. A pid is only a safe thing to signal while
/// the process it names is unreaped — once reaped, the number is free for the
/// kernel to reissue, and a destructive signal sent against it lands on whatever
/// unrelated process took it. Holding the unreaped `Child` here is what reserves
/// the identity: reaping happens through this same slot, under this same lock,
/// so there is no interval in which the slot names a process we no longer own.
///
/// The latch sits beside it because an executor makes a *sequence* of
/// invocations, and the slot is empty between them. Signalling whatever the slot
/// happened to hold made termination a moment: it could land in a gap, find
/// nothing, and let the very next invocation start and block unreachably. As a
/// latched state, an invocation published afterwards ends at its publication.
#[derive(Debug, Default)]
pub(crate) struct TmuxInvocationOwner {
    child: Mutex<Option<Child>>,
    /// Set once and never cleared: a generation told to end does not resume.
    terminating: AtomicBool,
}

pub(crate) type TmuxInvocationSlot = Arc<TmuxInvocationOwner>;

impl TmuxInvocationOwner {
    fn is_terminating(&self) -> bool {
        self.terminating.load(Ordering::Acquire)
    }
}

thread_local! {
    /// The slot this thread publishes its tmux invocations into.
    ///
    /// Thread-local because the generation's executor is the only thing that
    /// makes these invocations, and threading a handle through every pane helper
    /// would put supervision plumbing into call sites that have nothing to do
    /// with it. Absent for every other caller — the lifecycle primitives and the
    /// look path run outside any generation and are not fenced.
    static PUBLISHED_INVOCATIONS: RefCell<Option<TmuxInvocationSlot>> =
        const { RefCell::new(None) };
}

/// Publishes this thread's tmux invocations into `slot` for the rest of its
/// life. Called by a generation's executor as it starts.
pub(crate) fn publish_tmux_invocations(slot: TmuxInvocationSlot) {
    PUBLISHED_INVOCATIONS.with(|published| *published.borrow_mut() = Some(slot));
}

/// Signals whichever tmux client invocation `slot` currently holds.
///
/// The fence's forced step for tmux. Deliberately only the client: the tmux
/// **server** is not owned by the generation — it holds the operator's sessions,
/// and terminating it to fence one delivery would destroy the work the fence
/// exists to protect.
///
/// Latches the request first, then makes one non-blocking attempt to serve it,
/// as step 3 is contracted to be. Neither a failed `try_lock` nor an empty slot
/// loses the request: whoever holds the lock serves the latch as it reaps, and
/// whoever publishes next serves it on publication.
pub(crate) fn terminate_published_invocation(slot: &TmuxInvocationSlot) {
    slot.terminating.store(true, Ordering::Release);
    if let Ok(mut held) = slot.child.try_lock()
        && let Some(child) = held.as_mut()
    {
        let _ = child.kill();
    }
}

/// Owns one invocation for the duration of a wait: publishes the live child,
/// hands back the pipes to drain, and reaps through the slot on the way out.
struct PublishedInvocation {
    slot: Option<TmuxInvocationSlot>,
    /// Held only when no slot is published, since the child must still be
    /// reaped by someone.
    unpublished: Option<Child>,
}

impl PublishedInvocation {
    /// Takes ownership of `child`, publishing it if this thread has a slot.
    ///
    /// An invocation started behind an already-terminating generation is ended at
    /// once rather than refused: refusing would leave a spawned child owned by
    /// nobody, and the caller still needs something to reap.
    fn record(mut child: Child) -> (Self, Option<ChildStdout>, Option<ChildStderr>) {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let slot = PUBLISHED_INVOCATIONS.with(|published| published.borrow().clone());
        let owned = match slot {
            Some(slot) => {
                let published = match slot.child.lock() {
                    Ok(mut held) => {
                        *held = Some(child);
                        None
                    }
                    // A poisoned slot means the generation's supervision is
                    // already broken; keep ownership locally so the child is
                    // still reaped rather than leaked.
                    Err(_) => Some(child),
                };
                match published {
                    None => {
                        // Read after the lock is released, not under it. A check
                        // taken while holding the slot leaves the window where
                        // the forced step latches, fails its non-blocking attempt
                        // against this very publisher, and this publisher then
                        // releases without looking again — leaving the executor
                        // to block on a child nothing will signal. Reading here
                        // makes the two sides cover each other.
                        if slot.is_terminating() {
                            terminate_published_invocation(&slot);
                        }
                        Self {
                            slot: Some(slot),
                            unpublished: None,
                        }
                    }
                    Some(child) => Self {
                        slot: None,
                        unpublished: Some(child),
                    },
                }
            }
            None => Self {
                slot: None,
                unpublished: Some(child),
            },
        };
        (owned, stdout, stderr)
    }

    /// Reaps the invocation, keeping it reachable until its exit has actually
    /// been observed and clearing the slot in the same critical section as the
    /// reap.
    ///
    /// Polled through `try_wait` rather than parked in `wait`, because the wait
    /// is the interval that most needs the child to stay reachable. A tmux client
    /// can close its pipes and remain alive; draining then returns, and a
    /// blocking `wait` taken *after* removing the child from the slot left the
    /// executor stuck on a process the fence could no longer name. Holding the
    /// lock across a blocking `wait` would be no better — step 3 only ever tries
    /// for that lock, so it would fail and find nothing.
    ///
    /// The latch is re-served on every tick: a request that arrived while this
    /// loop held the lock is honoured here rather than lost.
    fn reap(mut self) -> io::Result<ExitStatus> {
        if let Some(mut child) = self.unpublished.take() {
            return child.wait();
        }
        let Some(slot) = self.slot.take() else {
            return Err(io::Error::other("tmux invocation slot poisoned"));
        };
        let poll = Duration::from_millis(INVOCATION_REAP_POLL_MS);
        loop {
            {
                let mut held = slot
                    .child
                    .lock()
                    .map_err(|_| io::Error::other("tmux invocation slot poisoned"))?;
                let child = held
                    .as_mut()
                    .ok_or_else(|| io::Error::other("tmux invocation was already reaped"))?;
                if slot.is_terminating() {
                    let _ = child.kill();
                }
                if let Some(status) = child.try_wait()? {
                    // Cleared only now that the process is reaped, so the slot
                    // never names a pid this process no longer owns.
                    *held = None;
                    return Ok(status);
                }
            }
            thread::sleep(poll);
        }
    }
}

/// Drains a terminated tmux invocation's pipes.
///
/// Reads stdout to EOF before stderr. Safe for this caller specifically: a tmux
/// client writes an error line to stderr or a result to stdout, never enough of
/// both to fill a pipe buffer while the other is still open. A future caller
/// that streams large volumes on both would need concurrent draining.
fn drain_invocation_pipes(
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    if let Some(mut stdout) = stdout {
        stdout.read_to_end(&mut out)?;
    }
    let mut err = Vec::new();
    if let Some(mut stderr) = stderr {
        stderr.read_to_end(&mut err)?;
    }
    Ok((out, err))
}

pub(crate) fn resolve_active_pane_target(
    tmux_socket: &Path,
    target_session: &str,
) -> Result<String, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["display-message", "-p", "-t", target_session, "#{pane_id}"],
    )?;
    let pane_target = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pane_target.is_empty() {
        return Err(format!(
            "tmux did not return an active pane for session {target_session}"
        ));
    }
    Ok(pane_target)
}

pub(crate) fn resolve_window_activity_marker(
    tmux_socket: &Path,
    pane_target: &str,
) -> Result<Option<String>, String> {
    let output = run_tmux_command_capture(
        tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            pane_target,
            "#{window_activity}",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("unknown format")
            || lower.contains("invalid format")
            || lower.contains("bad format")
        {
            return Ok(None);
        }
        if stderr.is_empty() {
            return Err("tmux display-message for window_activity failed".to_string());
        }
        return Err(stderr);
    }
    let marker = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if marker.is_empty() {
        return Ok(None);
    }
    Ok(Some(marker))
}

pub(crate) fn capture_pane_snapshot(
    tmux_socket: &Path,
    pane_target: &str,
) -> Result<String, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["capture-pane", "-p", "-t", pane_target, "-S", "-200"],
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn capture_pane_tail_lines(
    tmux_socket: &Path,
    pane_target: &str,
    requested_lines: usize,
) -> Result<Vec<String>, String> {
    let start = format!("-{LOOK_LINES_MAX}");
    let output = run_tmux_command(
        tmux_socket,
        &[
            "capture-pane",
            "-p",
            "-t",
            pane_target,
            "-S",
            start.as_str(),
        ],
    )?;
    let mut lines = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.len() > requested_lines {
        lines = lines.split_off(lines.len() - requested_lines);
    }
    Ok(lines)
}

pub(crate) fn resolve_cursor_column(
    tmux_socket: &Path,
    pane_target: &str,
) -> Result<usize, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["display-message", "-p", "-t", pane_target, "#{cursor_x}"],
    )?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    value
        .parse::<usize>()
        .map_err(|source| format!("failed to parse tmux cursor_x '{value}': {source}"))
}

pub(crate) fn inject_literal_text(
    tmux_socket: &Path,
    pane_target: &str,
    text: &str,
    append_enter: bool,
) -> Result<(), String> {
    if !text.is_empty() {
        paste_into_pane(tmux_socket, pane_target, text, PasteMode::Bracketed)?;
    }
    if append_enter {
        // Submit with an unbracketed carriage-return paste rather than
        // `send-keys Enter`. paste-buffer writes straight to the pane pty and
        // reaches the child even while the pane sits in copy-mode; `send-keys`
        // is routed through the copy-mode key table and silently swallowed
        // there. The body stays bracketed so multi-line content does not submit
        // early, and the submit is unbracketed so the bare CR is delivered as a
        // real Enter. Guarded by `append_enter` so a `raww` body-only write
        // (`no_enter=true`) injects no submit.
        paste_into_pane(tmux_socket, pane_target, "\r", PasteMode::Unbracketed)?;
    }
    Ok(())
}

enum PasteMode {
    Bracketed,
    Unbracketed,
}

fn paste_into_pane(
    tmux_socket: &Path,
    pane_target: &str,
    text: &str,
    mode: PasteMode,
) -> Result<(), String> {
    let buffer_name = next_paste_buffer_name();
    load_tmux_buffer(tmux_socket, &buffer_name, text)?;
    let mut arguments = vec!["paste-buffer", "-d"];
    if matches!(mode, PasteMode::Bracketed) {
        arguments.push("-p");
    }
    arguments.extend(["-b", buffer_name.as_str(), "-t", pane_target]);
    run_tmux_command(tmux_socket, &arguments)?;
    Ok(())
}

fn next_paste_buffer_name() -> String {
    let sequence = PASTE_BUFFER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{PASTE_BUFFER_NAME_PREFIX}-{pid}-{sequence}",
        pid = std::process::id()
    )
}

fn load_tmux_buffer(tmux_socket: &Path, buffer_name: &str, text: &str) -> Result<(), String> {
    let mut command = tmux_command(tmux_socket);
    command
        .args(["load-buffer", "-b", buffer_name, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|source| source.to_string())?;
    // Detach stdin and publish the child *before* writing a byte. The write can
    // park: a client that stops reading lets the pipe fill, and the executor
    // blocks in `write_all` with nothing left to interrupt it. Publishing after
    // the write left exactly that interval with an empty slot, so the fence's
    // forced step had nothing to signal — the one case it exists for.
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "failed to capture tmux load-buffer stdin".to_string())?;
    let (owned, stdout, stderr) = PublishedInvocation::record(child);
    let write_result = {
        let mut stdin = stdin;
        stdin
            .write_all(text.as_bytes())
            .map_err(|source| source.to_string())
        // Dropped here, closing the client's stdin so it sees EOF and exits.
    };
    // Reap before propagating a write failure. Returning early would drop the
    // guard without reaping, leaving a zombie and a stale entry in the slot that
    // a later forced step would signal.
    let drained = drain_invocation_pipes(stdout, stderr);
    let status = owned.reap().map_err(|source| source.to_string())?;
    write_result?;
    let (_stdout, stderr_bytes) = drained.map_err(|source| source.to_string())?;
    if status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
    if stderr.is_empty() {
        return Err("tmux load-buffer failed".to_string());
    }
    Err(stderr)
}

pub(crate) fn run_tmux_command(
    tmux_socket: &Path,
    command_arguments: &[impl AsRef<OsStr>],
) -> Result<std::process::Output, String> {
    let output = run_tmux_command_capture(tmux_socket, command_arguments)?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let command_name = command_arguments
        .first()
        .map(|argument| argument.as_ref().to_string_lossy().to_string())
        .unwrap_or_else(|| "tmux".to_string());
    if stderr.is_empty() {
        return Err(format!("tmux {command_name} failed"));
    }
    Err(stderr)
}

pub(crate) fn run_tmux_command_capture(
    tmux_socket: &Path,
    command_arguments: &[impl AsRef<OsStr>],
) -> Result<std::process::Output, String> {
    let mut command = tmux_command(tmux_socket);
    command
        .args(command_arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Spawn, publish, drain, reap — rather than `output()`, which does all of it
    // behind a handle nothing else can reach. The invocation has to be reachable
    // before the wait begins, because the whole point is to signal one the caller
    // is already blocked on.
    let child = command.spawn().map_err(|source| source.to_string())?;
    let (owned, stdout, stderr) = PublishedInvocation::record(child);
    let drained = drain_invocation_pipes(stdout, stderr);
    let status = owned.reap().map_err(|source| source.to_string())?;
    let (stdout, stderr) = drained.map_err(|source| source.to_string())?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Builds a tmux invocation addressing `tmux_socket` relative to its own
/// directory.
///
/// tmux binds and connects the `-S` path itself, so the `sockaddr_un` limit
/// applies to whatever string it is handed — and this is the longest path the
/// project constructs. Agentmux cannot address it through a directory
/// descriptor the way it does its own sockets, because the descriptor would
/// have to survive into tmux's process. Running the client from the socket's
/// directory achieves the same bound: the kernel resolves the bare file name
/// against the process working directory, so the address stays a handful of
/// bytes however deep the state root is.
///
/// Callers that create sessions must pass `-c` explicitly. tmux takes an
/// omitted start directory from the *client's* working directory, so moving the
/// client here would otherwise start panes in the bundle runtime directory.
fn tmux_command(tmux_socket: &Path) -> Command {
    let mut command = Command::new(resolve_tmux_program());
    match tmux_socket.parent().zip(tmux_socket.file_name()) {
        Some((directory, file_name)) => {
            command.current_dir(directory).arg("-S").arg(file_name);
        }
        None => {
            command.arg("-S").arg(tmux_socket);
        }
    }
    command
}

/// Resolves the tmux program to something the child's working directory cannot
/// reinterpret.
///
/// Two kinds of relative reference are affected by running the client from the
/// socket's directory. A value containing a separator is resolved by the kernel
/// against the *child's* working directory, so an `AGENTMUX_TMUX_COMMAND` of
/// `./wrapper.sh` would be looked for under the bundle runtime directory. A bare
/// name goes through `PATH`, whose entries may themselves be relative or empty
/// (an empty entry means the working directory), so `PATH=bin:/usr/bin` moves
/// with the child too.
///
/// Both are resolved here, against the relay's own working directory, and
/// nothing else is touched. In particular the child's `PATH` is left exactly as
/// inherited: it is not this function's to normalize, because the tmux client
/// hands its environment to a server it starts and thence to every pane, where
/// a relative entry is resolved against the *member's* directory by intent. The
/// scope of the fix is the program this code is about to execute.
fn resolve_tmux_program() -> std::ffi::OsString {
    let program = tmux_program();
    let path = Path::new(program.as_str());
    if path.components().count() > 1 {
        return std::path::absolute(path)
            .map(PathBuf::into_os_string)
            .unwrap_or_else(|_| program.clone().into());
    }
    resolve_program_on_search_path(program.as_str()).unwrap_or_else(|| program.into())
}

/// Resolves a bare program name against `PATH` from the current working
/// directory, the way `execvp` would from the child's.
///
/// Follows `execvp`'s search rather than approximating it: a candidate the
/// effective user cannot execute is passed over for a later entry rather than
/// selected and failed. Executability is asked of the kernel via `faccessat`
/// with `AT_EACCESS`, not inferred from mode bits — a file can carry an execute
/// bit that belongs to a principal this process is not, and an ACL can deny
/// where the mode appears to allow. The `is_file` guard is what keeps a
/// *searchable directory* of the same name from answering `X_OK`.
///
/// Returns `None` when `PATH` is unset — its libc default is absolute, so the
/// working directory cannot reach that lookup — and when no entry holds an
/// executable candidate, which leaves the bare name for `execvp` to fail on so
/// the operator sees an error naming what they configured.
fn resolve_program_on_search_path(program: &str) -> Option<std::ffi::OsString> {
    let search_path = std::env::var_os("PATH")?;
    std::env::split_paths(&search_path)
        .map(|entry| {
            if entry.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                entry
            }
        })
        .filter_map(|directory| std::path::absolute(directory.join(program)).ok())
        .find(|candidate| candidate.is_file() && effective_user_can_execute(candidate))
        .map(PathBuf::into_os_string)
}

/// Whether the *effective* user can execute `path`, as the kernel would decide
/// it at `exec` time.
fn effective_user_can_execute(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(candidate) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `candidate` is a valid NUL-terminated C string that outlives the
    // call, and `AT_FDCWD` needs no descriptor. `faccessat` only reads.
    let outcome = unsafe {
        libc::faccessat(
            libc::AT_FDCWD,
            candidate.as_ptr(),
            libc::X_OK,
            libc::AT_EACCESS,
        )
    };
    outcome == 0
}

fn tmux_program() -> String {
    std::env::var("AGENTMUX_TMUX_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "tmux".to_string())
}

pub(crate) fn sanitize_diagnostic_text(text: &str) -> String {
    const CHARS_MAX: usize = 512;
    let mut clipped = text.chars().take(CHARS_MAX).collect::<String>();
    if text.chars().count() > CHARS_MAX {
        clipped.push_str("...");
    }
    clipped
}
