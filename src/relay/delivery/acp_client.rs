use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::super::ACP_PROTOCOL_VERSION;

const ACP_CLIENT_NAME: &str = "agentmux-relay";
const ACP_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const ACP_LOAD_POST_RESPONSE_DRAIN_TIMEOUT: Duration = Duration::from_millis(200);
const ACP_PROMPT_POST_RESPONSE_DRAIN_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(super) enum AcpRequestError {
    Failed(String),
    Timeout(Duration),
    ConnectionClosed {
        reason: String,
        first_activity_observed: bool,
    },
}

#[derive(Debug)]
pub(super) struct AcpPromptCompletion {
    pub stop_reason: String,
    pub first_activity_observed: bool,
}

#[derive(Debug)]
pub(super) struct AcpRequestResult {
    pub result: Value,
    pub first_activity_observed: bool,
}

pub(super) struct AcpStdioClient {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    read_buffer: Vec<u8>,
    next_id: u64,
    snapshot_line_buffer: Vec<String>,
}

impl AcpStdioClient {
    pub(super) fn spawn(command_template: &str, working_directory: &Path) -> Result<Self, String> {
        let mut command = Command::new("sh");
        command
            .arg("-lc")
            .arg(command_template)
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|source| format!("spawn ACP stdio command failed: {source}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ACP stdio child stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ACP stdio child stdout unavailable".to_string())?;
        set_nonblocking(stdout.as_raw_fd(), true)?;
        Ok(Self {
            child,
            stdin,
            stdout,
            read_buffer: Vec::new(),
            next_id: 1,
            snapshot_line_buffer: Vec::new(),
        })
    }

    pub(super) fn initialize(&mut self) -> Result<Value, String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": {
                        "readTextFile": false,
                        "writeTextFile": false,
                    },
                    "terminal": false,
                },
                "clientInfo": {
                    "name": ACP_CLIENT_NAME,
                    "version": ACP_CLIENT_VERSION,
                },
            }),
            None,
            None,
            None,
            None,
        )
        .map(|value| value.result)
        .map_err(|error| match error {
            AcpRequestError::Failed(reason) => reason,
            AcpRequestError::Timeout(timeout) => {
                format!("ACP initialize timed out after {}ms", timeout.as_millis())
            }
            AcpRequestError::ConnectionClosed { reason, .. } => reason,
        })
    }

    pub(super) fn new_session(&mut self, working_directory: &Path) -> Result<String, String> {
        let result = self
            .request(
                "session/new",
                json!({
                    "cwd": working_directory.display().to_string(),
                    "mcpServers": [],
                }),
                None,
                None,
                Some(ACP_LOAD_POST_RESPONSE_DRAIN_TIMEOUT),
                None,
            )
            .map(|value| value.result)
            .map_err(|error| match error {
                AcpRequestError::Failed(reason) => reason,
                AcpRequestError::Timeout(timeout) => {
                    format!("ACP session/new timed out after {}ms", timeout.as_millis())
                }
                AcpRequestError::ConnectionClosed { reason, .. } => reason,
            })?;
        result
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| "ACP session/new response missing result.sessionId".to_string())
    }

    pub(super) fn load_session(
        &mut self,
        session_id: &str,
        working_directory: &Path,
    ) -> Result<(), String> {
        let _ = self
            .request(
                "session/load",
                json!({
                    "sessionId": session_id,
                    "cwd": working_directory.display().to_string(),
                    "mcpServers": [],
                }),
                None,
                None,
                Some(ACP_LOAD_POST_RESPONSE_DRAIN_TIMEOUT),
                None,
            )
            .map(|value| value.result)
            .map_err(|error| match error {
                AcpRequestError::Failed(reason) => reason,
                AcpRequestError::Timeout(timeout) => {
                    format!("ACP session/load timed out after {}ms", timeout.as_millis())
                }
                AcpRequestError::ConnectionClosed { reason, .. } => reason,
            })?;
        Ok(())
    }

    pub(super) fn prompt(
        &mut self,
        session_id: &str,
        prompt: &str,
        timeout: Option<Duration>,
        on_dispatched: Option<&mut dyn FnMut()>,
    ) -> Result<AcpPromptCompletion, AcpRequestError> {
        let result = self.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": prompt,
                    }
                ],
            }),
            timeout,
            Some(session_id),
            Some(ACP_PROMPT_POST_RESPONSE_DRAIN_TIMEOUT),
            on_dispatched,
        )?;
        result
            .result
            .get("stopReason")
            .and_then(Value::as_str)
            .map(|stop_reason| AcpPromptCompletion {
                stop_reason: stop_reason.to_string(),
                first_activity_observed: result.first_activity_observed,
            })
            .ok_or_else(|| {
                AcpRequestError::Failed(
                    "ACP session/prompt response missing result.stopReason".to_string(),
                )
            })
    }

    pub(super) fn take_snapshot_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.snapshot_line_buffer)
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
        prompt_session_id: Option<&str>,
        post_response_drain_timeout: Option<Duration>,
        mut on_dispatched: Option<&mut dyn FnMut()>,
    ) -> Result<AcpRequestResult, AcpRequestError> {
        let request_id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let message = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        }))
        .map_err(|source| {
            AcpRequestError::Failed(format!("serialize ACP request failed: {source}"))
        })?;
        self.stdin
            .write_all(message.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|source| {
                AcpRequestError::Failed(format!("write ACP request failed: {source}"))
            })?;
        if let Some(callback) = on_dispatched.as_mut() {
            callback();
        }

        let mut first_activity_observed = false;
        let mut read_timeout = timeout;
        loop {
            let line = match self.read_response_line(read_timeout) {
                Ok(line) => line,
                Err(AcpRequestError::Failed(reason)) => {
                    return Err(AcpRequestError::ConnectionClosed {
                        reason,
                        first_activity_observed,
                    });
                }
                Err(error) => return Err(error),
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let decoded = serde_json::from_str::<Value>(trimmed).map_err(|source| {
                AcpRequestError::Failed(format!("parse ACP response failed: {source}"))
            })?;
            if decoded.get("id") != Some(&json!(request_id)) {
                if (self.capture_update_snapshot_lines(&decoded, prompt_session_id)
                    || self.observe_permission_request_activity(&decoded, prompt_session_id))
                    && !first_activity_observed
                {
                    first_activity_observed = true;
                    read_timeout = None;
                }
                continue;
            }
            if let Some(error) = decoded.get("error") {
                return Err(AcpRequestError::Failed(error.to_string()));
            }
            if prompt_session_id.is_some() && !first_activity_observed {
                first_activity_observed = true;
            }
            if let Some(drain_timeout) = post_response_drain_timeout
                && self.drain_post_response_notifications(prompt_session_id, drain_timeout)
            {
                first_activity_observed = true;
            }
            return Ok(AcpRequestResult {
                result: decoded.get("result").cloned().unwrap_or(Value::Null),
                first_activity_observed,
            });
        }
    }

    fn drain_post_response_notifications(
        &mut self,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> bool {
        let mut observed = false;
        while let Ok(line) = self.read_response_line(Some(timeout)) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let decoded = match serde_json::from_str::<Value>(trimmed) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if self.capture_update_snapshot_lines(&decoded, session_id)
                || self.observe_permission_request_activity(&decoded, session_id)
            {
                observed = true;
            }
        }
        observed
    }

    fn capture_update_snapshot_lines(&mut self, value: &Value, session_id: Option<&str>) -> bool {
        if value.get("method").and_then(Value::as_str) != Some("session/update") {
            return false;
        }
        let params = value.get("params").unwrap_or(&Value::Null);
        if let Some(expected_session_id) = session_id
            && let Some(observed_session_id) = params.get("sessionId").and_then(Value::as_str)
            && observed_session_id != expected_session_id
        {
            return false;
        }
        collect_text_lines_from_value(params, &mut self.snapshot_line_buffer);
        true
    }

    fn observe_permission_request_activity(&self, value: &Value, session_id: Option<&str>) -> bool {
        if value.get("method").and_then(Value::as_str) != Some("session/request_permission") {
            return false;
        }
        let params = value.get("params").unwrap_or(&Value::Null);
        if let Some(expected_session_id) = session_id
            && let Some(observed_session_id) = params.get("sessionId").and_then(Value::as_str)
            && observed_session_id != expected_session_id
        {
            return false;
        }
        true
    }

    fn read_response_line(&mut self, timeout: Option<Duration>) -> Result<String, AcpRequestError> {
        let deadline = timeout.map(|value| Instant::now() + value);
        let mut chunk = [0_u8; 4096];
        loop {
            if let Some(newline_index) = self.read_buffer.iter().position(|value| *value == b'\n') {
                let mut line = self.read_buffer.drain(..=newline_index).collect::<Vec<_>>();
                if matches!(line.last(), Some(b'\n')) {
                    line.pop();
                }
                if matches!(line.last(), Some(b'\r')) {
                    line.pop();
                }
                return String::from_utf8(line).map_err(|source| {
                    AcpRequestError::Failed(format!("decode ACP response failed: {source}"))
                });
            }

            match self.stdout.read(&mut chunk) {
                Ok(0) => {
                    let exit_code = self
                        .child
                        .try_wait()
                        .ok()
                        .flatten()
                        .and_then(|status| status.code());
                    return Err(AcpRequestError::Failed(format!(
                        "ACP peer closed stdout (exit_code={exit_code:?})"
                    )));
                }
                Ok(count) => self.read_buffer.extend_from_slice(&chunk[..count]),
                Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Some(limit) = deadline
                        && Instant::now() >= limit
                    {
                        return Err(AcpRequestError::Timeout(
                            timeout.unwrap_or(Duration::from_millis(0)),
                        ));
                    }
                    if let Ok(Some(status)) = self.child.try_wait() {
                        return Err(AcpRequestError::Failed(format!(
                            "ACP peer exited before response (exit_code={:?})",
                            status.code()
                        )));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(source) => {
                    return Err(AcpRequestError::Failed(format!(
                        "read ACP response failed: {source}"
                    )));
                }
            }
        }
    }
}

fn collect_text_lines_from_value(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_text_lines_from_value(value, output);
            }
        }
        Value::Object(values) => {
            if let Some(text) = values.get("text").and_then(Value::as_str) {
                append_text_lines(text, output);
            }
            for value in values.values() {
                collect_text_lines_from_value(value, output);
            }
        }
        _ => {}
    }
}

fn append_text_lines(text: &str, output: &mut Vec<String>) {
    for line in text.split('\n') {
        let normalized = line.trim_end_matches('\r');
        if !normalized.is_empty() {
            output.push(normalized.to_string());
        }
    }
}

fn set_nonblocking(file_descriptor: i32, enable: bool) -> Result<(), String> {
    // SAFETY: `fcntl` is called with a live file descriptor owned by this
    // process. The command and arguments follow libc contract.
    let flags = unsafe { libc::fcntl(file_descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let updated_flags = if enable {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    // SAFETY: `fcntl` receives the same valid descriptor and bitflag payload.
    let result = unsafe { libc::fcntl(file_descriptor, libc::F_SETFL, updated_flags) };
    if result < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
