use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::{Value, json};

use super::{PROTOCOL_VERSION, PermissionOption, PermissionRequest, ReplayEntry};
use crate::runtime::inscriptions::emit_inscription;

const ACP_CLIENT_NAME: &str = "agentmux-relay";
const ACP_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const REPLAY_BUFFER_MAX_ENTRIES: usize = 1000;

pub(in crate::acp) fn append_replay_entries(
    buffer: &mut Vec<ReplayEntry>,
    entries: Vec<ReplayEntry>,
) {
    buffer.extend(entries);
    if buffer.len() > REPLAY_BUFFER_MAX_ENTRIES {
        let overflow = buffer.len() - REPLAY_BUFFER_MAX_ENTRIES;
        buffer.drain(0..overflow);
    }
}

pub type DispatchHandler = Box<dyn FnOnce() + Send + 'static>;
pub type PermissionHandler = Box<dyn FnMut(&PermissionRequest) -> Option<String> + Send + 'static>;
pub type PromptCompletionHandler = Box<dyn FnOnce(PromptCompletion) + Send + 'static>;

#[derive(Debug)]
pub enum AcpRequestError {
    Failed(String),
    Timeout(Duration),
    ConnectionClosed { reason: String },
    TransportUnavailable { reason: String },
}

#[derive(Debug)]
pub enum PromptCompletion {
    Completed { stop_reason: String },
    ProtocolError(String),
    ConnectionClosed { reason: String },
}

#[derive(Debug)]
pub enum PromptDispatchOutcome {
    Submitted,
    TransportUnavailable { reason: String },
    SerializationFailed(String),
}

#[derive(Debug)]
enum ResponseEnvelope {
    Result(Value),
    Error(String),
}

pub(in crate::acp) type SharedStdin = Arc<Mutex<ChildStdin>>;
type SharedReplay = Arc<Mutex<Vec<ReplayEntry>>>;
type SharedPending = Arc<Mutex<HashMap<u64, mpsc::Sender<ResponseEnvelope>>>>;

struct ActivePrompt {
    session_id: String,
    request_id: u64,
    on_permission_request: Mutex<Option<PermissionHandler>>,
    on_completion: Mutex<Option<PromptCompletionHandler>>,
    // Dropped when the active_prompt slot is cleared (after on_completion
    // fires, or on synchronous dispatch failure). The matching receiver
    // on `AcpStdioClient.last_prompt_signal` then observes `Disconnected`
    // and `wait_for_prompt_complete()` returns. Never `.send()`'d; presence
    // of the sender is itself the "still in flight" signal.
    _completion_signal: mpsc::Sender<()>,
}

type SharedActivePrompt = Arc<Mutex<Option<Arc<ActivePrompt>>>>;

pub struct AcpStdioClient {
    child: Child,
    stdin: SharedStdin,
    replay_buffer: SharedReplay,
    pending_responses: SharedPending,
    active_prompt: SharedActivePrompt,
    reader_handle: Option<JoinHandle<()>>,
    next_id: u64,
    last_prompt_signal: Mutex<Option<mpsc::Receiver<()>>>,
}

fn write_line_to_stdin(stdin: &SharedStdin, payload: &str) -> io::Result<()> {
    let mut guard = stdin
        .lock()
        .map_err(|_| io::Error::other("ACP stdin mutex poisoned"))?;
    guard.write_all(payload.as_bytes())?;
    guard.write_all(b"\n")?;
    guard.flush()
}

impl AcpStdioClient {
    // Spawn the ACP agent directly (no shell middleman). The command
    // template is split on whitespace. Environment variables are passed
    // explicitly via the `environment` parameter.
    //
    // TODO: Consider shell-word parsing (e.g. shell-words crate) for
    //       templates containing metacharacters ($, |, &&, backticks).
    pub fn spawn(
        command_template: &str,
        working_directory: &Path,
        environment: &[(String, String)],
    ) -> Result<Self, String> {
        let parts: Vec<&str> = command_template.split_whitespace().collect();
        if parts.is_empty() {
            return Err("ACP command template is empty".to_string());
        }
        let mut command = Command::new(parts[0]);
        command
            .args(&parts[1..])
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in environment {
            command.env(key, value);
        }
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

        let stdin = Arc::new(Mutex::new(stdin));
        let replay_buffer: SharedReplay = Arc::new(Mutex::new(Vec::new()));
        let pending_responses: SharedPending = Arc::new(Mutex::new(HashMap::new()));
        let active_prompt: SharedActivePrompt = Arc::new(Mutex::new(None));

        let reader_handle = spawn_reader_thread(
            BufReader::new(stdout),
            Arc::clone(&stdin),
            Arc::clone(&replay_buffer),
            Arc::clone(&pending_responses),
            Arc::clone(&active_prompt),
        );

        Ok(Self {
            child,
            stdin,
            replay_buffer,
            pending_responses,
            active_prompt,
            reader_handle: Some(reader_handle),
            next_id: 1,
            last_prompt_signal: Mutex::new(None),
        })
    }

    pub fn initialize(&mut self) -> Result<Value, String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
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
        )
        .map_err(|error| match error {
            AcpRequestError::Failed(reason) => reason,
            AcpRequestError::Timeout(timeout) => {
                format!("ACP initialize timed out after {}ms", timeout.as_millis())
            }
            AcpRequestError::ConnectionClosed { reason } => reason,
            AcpRequestError::TransportUnavailable { reason } => reason,
        })
    }

    pub fn new_session(&mut self, working_directory: &Path) -> Result<String, String> {
        let result = self
            .request(
                "session/new",
                json!({
                    "cwd": working_directory.display().to_string(),
                    "mcpServers": [],
                }),
                None,
            )
            .map_err(|error| match error {
                AcpRequestError::Failed(reason) => reason,
                AcpRequestError::Timeout(timeout) => {
                    format!("ACP session/new timed out after {}ms", timeout.as_millis())
                }
                AcpRequestError::ConnectionClosed { reason } => reason,
                AcpRequestError::TransportUnavailable { reason } => reason,
            })?;
        result
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| "ACP session/new response missing result.sessionId".to_string())
    }

    pub fn load_session(
        &mut self,
        session_id: &str,
        working_directory: &Path,
    ) -> Result<Vec<ReplayEntry>, String> {
        self.load_session_with_timeout(session_id, working_directory, None)
    }

    pub fn load_session_with_timeout(
        &mut self,
        session_id: &str,
        working_directory: &Path,
        timeout: Option<Duration>,
    ) -> Result<Vec<ReplayEntry>, String> {
        let entries_before_load = self
            .replay_buffer
            .lock()
            .expect("replay_buffer mutex")
            .len();
        self.request(
            "session/load",
            json!({
                "sessionId": session_id,
                "cwd": working_directory.display().to_string(),
                "mcpServers": [],
            }),
            timeout,
        )
        .map_err(|error| match error {
            AcpRequestError::Failed(reason) => reason,
            AcpRequestError::Timeout(timeout) => {
                format!("ACP session/load timed out after {}ms", timeout.as_millis())
            }
            AcpRequestError::ConnectionClosed { reason } => reason,
            AcpRequestError::TransportUnavailable { reason } => reason,
        })?;
        let buffer = self.replay_buffer.lock().expect("replay_buffer mutex");
        Ok(buffer[entries_before_load..].to_vec())
    }

    pub fn prompt(
        &mut self,
        session_id: &str,
        prompt: &str,
        on_dispatched: Option<DispatchHandler>,
        on_permission_request: Option<PermissionHandler>,
        on_completion: PromptCompletionHandler,
    ) -> PromptDispatchOutcome {
        let request_id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let (signal_tx, signal_rx) = mpsc::channel::<()>();
        let active = Arc::new(ActivePrompt {
            session_id: session_id.to_string(),
            request_id,
            on_permission_request: Mutex::new(on_permission_request),
            on_completion: Mutex::new(Some(on_completion)),
            _completion_signal: signal_tx,
        });
        {
            let mut slot = self.active_prompt.lock().expect("active_prompt mutex");
            if slot.is_some() {
                return PromptDispatchOutcome::SerializationFailed(
                    "ACP prompt already in flight".to_string(),
                );
            }
            *slot = Some(Arc::clone(&active));
        }
        *self
            .last_prompt_signal
            .lock()
            .expect("last_prompt_signal mutex") = Some(signal_rx);

        let message = match serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": prompt,
                    }
                ],
            },
        })) {
            Ok(message) => message,
            Err(source) => {
                *self.active_prompt.lock().expect("active_prompt mutex") = None;
                return PromptDispatchOutcome::SerializationFailed(format!(
                    "serialize ACP prompt failed: {source}"
                ));
            }
        };

        if let Err(source) = write_line_to_stdin(&self.stdin, message.as_str()) {
            *self.active_prompt.lock().expect("active_prompt mutex") = None;
            return PromptDispatchOutcome::TransportUnavailable {
                reason: format!("write ACP prompt failed: {source}"),
            };
        }

        let mut user_lines: Vec<String> = Vec::new();
        append_text_lines(prompt, &mut user_lines);
        if !user_lines.is_empty() {
            let mut buffer = self.replay_buffer.lock().expect("replay_buffer mutex");
            append_replay_entries(&mut buffer, vec![ReplayEntry::User { lines: user_lines }]);
        }

        if let Some(callback) = on_dispatched {
            callback();
        }

        PromptDispatchOutcome::Submitted
    }

    pub fn read_replay_entries(&self) -> Vec<ReplayEntry> {
        self.replay_buffer
            .lock()
            .expect("replay_buffer mutex")
            .clone()
    }

    pub fn replay_buffer_handle(&self) -> Arc<Mutex<Vec<ReplayEntry>>> {
        Arc::clone(&self.replay_buffer)
    }

    // Block until the most recent `prompt()` call has fully completed -- either
    // the background reader fired its `on_completion` handler (response arrived
    // or transport closed) or the synchronous dispatch path itself failed and
    // cleared the active prompt. Returns immediately if no prompt has been
    // submitted since the last wait. Used by the per-target worker thread to
    // serialize the single-flight ACP prompt invariant after a fire-and-forget
    // dispatch.
    pub fn wait_for_prompt_complete(&self) {
        let receiver = self
            .last_prompt_signal
            .lock()
            .expect("last_prompt_signal mutex")
            .take();
        if let Some(receiver) = receiver {
            let _ = receiver.recv();
        }
    }

    pub fn replay_entries_since(&self, cursor: usize) -> (Vec<ReplayEntry>, usize) {
        let buffer = self.replay_buffer.lock().expect("replay_buffer mutex");
        let len = buffer.len();
        if cursor >= len {
            return (Vec::new(), len);
        }
        (buffer[cursor..].to_vec(), len)
    }

    pub fn child_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }

    pub fn shutdown(&mut self) {
        // Killing the child closes its stdout, which makes the reader's
        // blocking `read_line` return EOF (Ok(0)) and exit the loop. The
        // reader then drops its clones of the shared stdin, replay buffer,
        // and pending-response registry; pending senders dropped in the
        // registry signal `Disconnected` to any waiters in `prompt`/`request`.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, AcpRequestError> {
        let request_id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let (tx, rx) = mpsc::channel::<ResponseEnvelope>();
        {
            let mut pending = self.pending_responses.lock().expect("pending mutex");
            pending.insert(request_id, tx);
        }
        let message = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        }))
        .map_err(|source| {
            self.pending_responses
                .lock()
                .expect("pending mutex")
                .remove(&request_id);
            AcpRequestError::Failed(format!("serialize ACP request failed: {source}"))
        })?;
        if let Err(source) = write_line_to_stdin(&self.stdin, message.as_str()) {
            self.pending_responses
                .lock()
                .expect("pending mutex")
                .remove(&request_id);
            return Err(AcpRequestError::TransportUnavailable {
                reason: format!("write ACP request failed: {source}"),
            });
        }
        let envelope = match timeout {
            Some(deadline) => match rx.recv_timeout(deadline) {
                Ok(envelope) => envelope,
                Err(RecvTimeoutError::Timeout) => {
                    self.pending_responses
                        .lock()
                        .expect("pending mutex")
                        .remove(&request_id);
                    return Err(AcpRequestError::Timeout(deadline));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(AcpRequestError::ConnectionClosed {
                        reason: "ACP transport closed before response".to_string(),
                    });
                }
            },
            None => match rx.recv() {
                Ok(envelope) => envelope,
                Err(_) => {
                    return Err(AcpRequestError::ConnectionClosed {
                        reason: "ACP transport closed before response".to_string(),
                    });
                }
            },
        };
        match envelope {
            ResponseEnvelope::Result(value) => Ok(value),
            ResponseEnvelope::Error(reason) => Err(AcpRequestError::Failed(reason)),
        }
    }
}

fn spawn_reader_thread(
    reader: BufReader<ChildStdout>,
    stdin: SharedStdin,
    replay_buffer: SharedReplay,
    pending_responses: SharedPending,
    active_prompt: SharedActivePrompt,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("acp-reader".to_string())
        .spawn(move || {
            run_reader_loop(
                reader,
                &stdin,
                &replay_buffer,
                &pending_responses,
                &active_prompt,
            );
        })
        .expect("spawn ACP reader thread")
}

fn run_reader_loop(
    mut reader: BufReader<ChildStdout>,
    stdin: &SharedStdin,
    replay_buffer: &SharedReplay,
    pending_responses: &SharedPending,
    active_prompt: &SharedActivePrompt,
) {
    let mut pending_tool_calls: HashMap<String, ReplayEntry> = HashMap::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(source) => {
                emit_inscription(
                    "acp.reader.read_failed",
                    &json!({"cause": source.to_string()}),
                );
                break;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let decoded = match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => value,
            Err(source) => {
                emit_inscription(
                    "acp.reader.parse_failed",
                    &json!({"line": trimmed, "cause": source.to_string()}),
                );
                continue;
            }
        };

        if let Some(method) = decoded.get("method").and_then(Value::as_str) {
            match method {
                "session/update" => {
                    dispatch_session_update(&decoded, replay_buffer, &mut pending_tool_calls)
                }
                "session/request_permission" => {
                    dispatch_permission_request(&decoded, active_prompt, stdin)
                }
                other => emit_inscription("acp.reader.unknown_method", &json!({"method": other})),
            }
            continue;
        }

        if let Some(id) = decoded.get("id").and_then(Value::as_u64) {
            if try_dispatch_prompt_response(&decoded, id, active_prompt) {
                continue;
            }
            let envelope = if let Some(error) = decoded.get("error") {
                ResponseEnvelope::Error(error.to_string())
            } else {
                ResponseEnvelope::Result(decoded.get("result").cloned().unwrap_or(Value::Null))
            };
            let sender = pending_responses.lock().expect("pending mutex").remove(&id);
            match sender {
                Some(sender) => {
                    let _ = sender.send(envelope);
                }
                None => emit_inscription("acp.reader.orphan_response", &json!({"id": id})),
            }
            continue;
        }

        emit_inscription("acp.reader.unrecognized_message", &json!({"line": trimmed}));
    }

    pending_responses.lock().expect("pending mutex").clear();
    let active_on_exit = active_prompt.lock().expect("active_prompt mutex").take();
    if let Some(active) = active_on_exit {
        let handler = active
            .on_completion
            .lock()
            .expect("on_completion mutex")
            .take();
        if let Some(handler) = handler {
            handler(PromptCompletion::ConnectionClosed {
                reason: "ACP transport closed before response".to_string(),
            });
        }
    }
}

fn try_dispatch_prompt_response(
    decoded: &Value,
    id: u64,
    active_prompt: &SharedActivePrompt,
) -> bool {
    let active_clone = {
        let slot = active_prompt.lock().expect("active_prompt mutex");
        match slot.as_ref() {
            Some(active) if active.request_id == id => Some(Arc::clone(active)),
            _ => None,
        }
    };
    let Some(active) = active_clone else {
        return false;
    };
    let completion = if let Some(error) = decoded.get("error") {
        PromptCompletion::ProtocolError(error.to_string())
    } else {
        let stop_reason = decoded
            .get("result")
            .and_then(|result| result.get("stopReason"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        match stop_reason {
            Some(stop_reason) => PromptCompletion::Completed { stop_reason },
            None => PromptCompletion::ProtocolError(
                "ACP session/prompt response missing result.stopReason".to_string(),
            ),
        }
    };
    let handler = active
        .on_completion
        .lock()
        .expect("on_completion mutex")
        .take();
    *active_prompt.lock().expect("active_prompt mutex") = None;
    if let Some(handler) = handler {
        handler(completion);
    }
    true
}

fn dispatch_session_update(
    decoded: &Value,
    replay_buffer: &SharedReplay,
    pending_tool_calls: &mut HashMap<String, ReplayEntry>,
) {
    let params = decoded.get("params").unwrap_or(&Value::Null);
    let entries = parse_replay_entries_from_params(params, pending_tool_calls);
    if !entries.is_empty() {
        let mut buffer = replay_buffer.lock().expect("replay_buffer mutex");
        append_replay_entries(&mut buffer, entries);
    }
}

fn dispatch_permission_request(
    decoded: &Value,
    active_prompt: &SharedActivePrompt,
    stdin: &SharedStdin,
) {
    let request_id = match decoded.get("id").and_then(Value::as_u64) {
        Some(id) => id,
        None => {
            emit_inscription("acp.reader.permission_request_missing_id", &json!({}));
            return;
        }
    };
    let params = decoded.get("params").unwrap_or(&Value::Null);
    let session_id = params.get("sessionId").and_then(Value::as_str);

    let active_clone = active_prompt
        .lock()
        .expect("active_prompt mutex")
        .as_ref()
        .map(Arc::clone);
    let active = match active_clone {
        Some(active) => active,
        None => {
            emit_inscription(
                "acp.reader.permission_dropped_no_active_prompt",
                &json!({"id": request_id}),
            );
            send_permission_response(stdin, request_id, None);
            return;
        }
    };

    if session_id.is_some_and(|sid| sid != active.session_id.as_str()) {
        emit_inscription(
            "acp.reader.permission_dropped_session_mismatch",
            &json!({"id": request_id}),
        );
        send_permission_response(stdin, request_id, None);
        return;
    }

    let request = build_permission_request_from_params(params, request_id);

    let decision = {
        let mut handler_slot = active
            .on_permission_request
            .lock()
            .expect("permission mutex");
        match handler_slot.as_mut() {
            Some(handler) => handler(&request),
            None => None,
        }
    };

    send_permission_response(stdin, request_id, decision);
}

fn build_permission_request_from_params(params: &Value, request_id: u64) -> PermissionRequest {
    let tool_call_title = params
        .get("toolCall")
        .and_then(|tc| tc.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("unknown tool")
        .to_string();
    let options: Vec<PermissionOption> = params
        .get("options")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|opt| {
                    Some(PermissionOption {
                        option_id: opt.get("optionId")?.as_str()?.to_string(),
                        name: opt.get("name")?.as_str()?.to_string(),
                        kind: opt
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    PermissionRequest {
        request_id,
        tool_call_title,
        requested_kind: params
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .to_string(),
        requested_details: params.clone(),
        options,
    }
}

fn send_permission_response(
    stdin: &SharedStdin,
    request_id: u64,
    selected_option_id: Option<String>,
) {
    let outcome = match selected_option_id {
        Some(option_id) => json!({"outcome": "selected", "optionId": option_id}),
        None => json!({"outcome": "cancelled"}),
    };
    let response = match serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {"outcome": outcome},
    })) {
        Ok(value) => value,
        Err(source) => {
            emit_inscription(
                "acp.reader.permission_response_serialize_failed",
                &json!({"cause": source.to_string()}),
            );
            return;
        }
    };
    if let Err(source) = write_line_to_stdin(stdin, response.as_str()) {
        emit_inscription(
            "acp.reader.permission_response_write_failed",
            &json!({"cause": source.to_string()}),
        );
    }
}

pub(super) fn parse_replay_entries_from_params(
    params: &Value,
    pending_calls: &mut HashMap<String, ReplayEntry>,
) -> Vec<ReplayEntry> {
    let update_field = params.get("update").unwrap_or(&Value::Null);
    let updates: Vec<&Value> = match update_field.as_array() {
        Some(arr) => arr.iter().collect(),
        None if !update_field.is_null() => vec![update_field],
        None => return Vec::new(),
    };
    let mut entries = Vec::with_capacity(updates.len());
    for update in updates {
        let Some(update_kind) = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .map(String::from)
        else {
            emit_inscription(
                "acp.reader.session_update_missing_kind",
                &json!({"update": update}),
            );
            continue;
        };
        match update_kind.as_str() {
            "user_message_chunk" => {
                let lines = collect_text_lines_from_value(update);
                if !lines.is_empty() {
                    entries.push(ReplayEntry::User { lines });
                }
            }
            "agent_message_chunk" => {
                let lines = collect_text_lines_from_value(update);
                if !lines.is_empty() {
                    entries.push(ReplayEntry::Agent { lines });
                }
            }
            "agent_thought_chunk" => {
                let lines = collect_text_lines_from_value(update);
                if !lines.is_empty() {
                    entries.push(ReplayEntry::Cognition { lines });
                }
            }
            "tool_call" => {
                let Some(call_id) = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .map(String::from)
                else {
                    emit_inscription(
                        "acp.reader.tool_call_missing_id",
                        &json!({"update": update}),
                    );
                    continue;
                };
                let invocation = update.clone();
                entries.push(ReplayEntry::Invocation {
                    call_id: call_id.clone(),
                    status: super::ToolCallStatus::Pending,
                    invocation,
                    result: None,
                });
                pending_calls.insert(call_id, entries.last().unwrap().clone());
            }
            "tool_call_update" => {
                let Some(call_id) = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .map(String::from)
                else {
                    emit_inscription(
                        "acp.reader.tool_call_update_missing_id",
                        &json!({"update": update}),
                    );
                    continue;
                };
                let result = update.clone();
                if let Some(ReplayEntry::Invocation {
                    status,
                    result: existing_result,
                    call_id: existing_call_id,
                    invocation,
                }) = pending_calls.get_mut(&call_id)
                {
                    *status = super::ToolCallStatus::Completed;
                    let result_clone = result.clone();
                    *existing_result = Some(result_clone);
                    let completed = ReplayEntry::Invocation {
                        call_id: existing_call_id.clone(),
                        status: super::ToolCallStatus::Completed,
                        invocation: invocation.clone(),
                        result: Some(result),
                    };
                    entries.push(completed);
                } else {
                    entries.push(ReplayEntry::Invocation {
                        call_id,
                        status: super::ToolCallStatus::Completed,
                        invocation: serde_json::json!({}),
                        result: Some(result),
                    });
                }
            }
            _ => entries.push(ReplayEntry::Update {
                update_kind,
                lines: collect_text_lines_from_value(update),
            }),
        }
    }
    entries
}

impl Drop for AcpStdioClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn collect_text_lines_from_value(value: &Value) -> Vec<String> {
    let mut output = Vec::new();
    collect_text_lines_recursive(value, &mut output);
    output
}

fn collect_text_lines_recursive(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_text_lines_recursive(value, output);
            }
        }
        Value::Object(values) => {
            if let Some(text) = values.get("text").and_then(Value::as_str) {
                append_text_lines(text, output);
            }
            for value in values.values() {
                collect_text_lines_recursive(value, output);
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
