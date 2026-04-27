use std::{ffi::OsStr, path::Path, process::Command};

use serde_json::Value;

use crate::runtime::inscriptions::emit_inscription;

const DELIVERY_DIAGNOSTICS_ENVVAR: &str = "AGENTMUX_RELAY_DELIVERY_DIAGNOSTICS";
const SEND_KEYS_CHUNK_BYTES: usize = 1024;
const LOOK_LINES_MAX: usize = 1000;

pub(super) fn resolve_active_pane_target(
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

pub(super) fn resolve_window_activity_marker(
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

pub(super) fn operator_interaction_active(
    tmux_socket: &Path,
    target_session: &str,
    pane_target: &str,
) -> Result<Option<String>, String> {
    if pane_in_mode_active(tmux_socket, pane_target)? {
        return Ok(Some("pane_in_mode".to_string()));
    }
    if let Some(table) = active_client_key_table(tmux_socket, target_session)? {
        return Ok(Some(format!("client_key_table={table}")));
    }
    Ok(None)
}

fn pane_in_mode_active(tmux_socket: &Path, pane_target: &str) -> Result<bool, String> {
    let output = run_tmux_command_capture(
        tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            pane_target,
            "#{pane_in_mode}",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("unknown format")
            || lower.contains("invalid format")
            || lower.contains("bad format")
        {
            return Ok(false);
        }
        if stderr.is_empty() {
            return Err("tmux display-message for pane_in_mode failed".to_string());
        }
        return Err(stderr);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "1")
}

fn active_client_key_table(
    tmux_socket: &Path,
    target_session: &str,
) -> Result<Option<String>, String> {
    let output = run_tmux_command_capture(
        tmux_socket,
        &[
            "list-clients",
            "-t",
            target_session,
            "-F",
            "#{client_key_table}",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("no current client")
            || lower.contains("unknown command")
            || lower.contains("unsupported")
            || lower.contains("unknown format")
            || lower.contains("invalid format")
            || lower.contains("bad format")
        {
            return Ok(None);
        }
        if stderr.is_empty() {
            return Err("tmux list-clients for key table failed".to_string());
        }
        return Err(stderr);
    }
    let active = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|value| !value.is_empty() && *value != "root")
        .map(ToOwned::to_owned);
    Ok(active)
}

pub(super) fn capture_pane_snapshot(
    tmux_socket: &Path,
    pane_target: &str,
) -> Result<String, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["capture-pane", "-p", "-t", pane_target, "-S", "-200"],
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(super) fn capture_pane_tail_lines(
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

pub(super) fn resolve_cursor_column(
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

pub(super) fn inject_prompt(
    tmux_socket: &Path,
    pane_target: &str,
    prompt: &str,
) -> Result<(), String> {
    inject_literal_text(tmux_socket, pane_target, prompt, true)
}

pub(super) fn inject_literal_text(
    tmux_socket: &Path,
    pane_target: &str,
    text: &str,
    append_enter: bool,
) -> Result<(), String> {
    for chunk in split_send_keys_chunks(text, SEND_KEYS_CHUNK_BYTES) {
        run_tmux_command(
            tmux_socket,
            &["send-keys", "-l", "-t", pane_target, "--", chunk.as_str()],
        )?;
    }
    if append_enter {
        run_tmux_command(tmux_socket, &["send-keys", "-t", pane_target, "Enter"])?;
    }
    Ok(())
}

fn split_send_keys_chunks(text: &str, max_bytes: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let max_bytes = max_bytes.max(1);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut current_bytes = 0usize;
    for (index, ch) in text.char_indices() {
        let ch_bytes = ch.len_utf8();
        if current_bytes != 0 && current_bytes + ch_bytes > max_bytes {
            chunks.push(text[start..index].to_string());
            start = index;
            current_bytes = 0;
        }
        current_bytes += ch_bytes;
    }
    if start < text.len() {
        chunks.push(text[start..].to_string());
    }
    chunks
}

pub(super) fn run_tmux_command(
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

pub(super) fn run_tmux_command_capture(
    tmux_socket: &Path,
    command_arguments: &[impl AsRef<OsStr>],
) -> Result<std::process::Output, String> {
    let mut command = Command::new(tmux_program());
    command.arg("-S").arg(tmux_socket).args(command_arguments);
    command.output().map_err(|source| source.to_string())
}

fn tmux_program() -> String {
    std::env::var("AGENTMUX_TMUX_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "tmux".to_string())
}

pub(super) fn sanitize_diagnostic_text(text: &str) -> String {
    const CHARS_MAX: usize = 512;
    let mut clipped = text.chars().take(CHARS_MAX).collect::<String>();
    if text.chars().count() > CHARS_MAX {
        clipped.push_str("...");
    }
    clipped
}

pub(super) fn emit_delivery_diagnostic(event: &str, details: &Value) {
    if !delivery_diagnostics_enabled() {
        return;
    }
    emit_inscription(format!("relay.{event}").as_str(), details);
}

fn delivery_diagnostics_enabled() -> bool {
    std::env::var(DELIVERY_DIAGNOSTICS_ENVVAR)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}
