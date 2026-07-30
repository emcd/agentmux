use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

const PASTE_BUFFER_NAME_PREFIX: &str = "agentmux-relay";
const LOOK_LINES_MAX: usize = 1000;

static PASTE_BUFFER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to capture tmux load-buffer stdin".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|source| source.to_string())?;
    }
    let output = child
        .wait_with_output()
        .map_err(|source| source.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
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
    command.args(command_arguments);
    command.output().map_err(|source| source.to_string())
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

/// Resolves the tmux program to something the working directory cannot
/// reinterpret.
///
/// Running the client from the socket's directory changes what a relative
/// program reference means, and both kinds of reference are affected. A value
/// containing a separator is resolved by the kernel against the *child's*
/// working directory, so an `AGENTMUX_TMUX_COMMAND` of `./wrapper.sh` would be
/// looked for under the bundle runtime directory instead of where the relay was
/// launched. A bare name goes through `PATH`, whose entries may themselves be
/// relative or empty (an empty entry means the working directory), so
/// `PATH=bin:/usr/bin` moves with the child too.
///
/// Both are pinned here, before the working directory changes, by resolving
/// against the relay's own. A lookup that finds nothing falls back to the
/// configured value unchanged, so the failure an operator sees is still the
/// exec error naming what they configured.
fn resolve_tmux_program() -> std::ffi::OsString {
    let program = tmux_program();
    let path = Path::new(program.as_str());
    if path.components().count() > 1 {
        return std::path::absolute(path)
            .map(PathBuf::into_os_string)
            .unwrap_or_else(|_| program.clone().into());
    }
    resolve_program_on_path(program.as_str()).unwrap_or_else(|| program.into())
}

/// Resolves a bare program name against `PATH` the way `execvp` would, but from
/// the current working directory rather than the child's.
///
/// Returns `None` when `PATH` is unset (its libc default is absolute, so the
/// working directory cannot affect that lookup) or when no entry holds a
/// matching executable.
fn resolve_program_on_path(program: &str) -> Option<std::ffi::OsString> {
    let search_path = std::env::var_os("PATH")?;
    std::env::split_paths(&search_path)
        .map(|entry| {
            // An empty `PATH` entry means the working directory, which is exactly
            // the reference this resolution exists to pin.
            if entry.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                entry
            }
        })
        .filter_map(|directory| std::path::absolute(directory.join(program)).ok())
        .find(|candidate| is_executable_file(candidate))
        .map(PathBuf::into_os_string)
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0)
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
