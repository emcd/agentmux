use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::{Value, json};

use super::{
    PROTOCOL_VERSION, PendingToolCall, PermissionOption, PermissionRequest, ReplayEntry, UserSource,
};
use crate::runtime::inscriptions::emit_inscription;

const ACP_CLIENT_NAME: &str = "agentmux-relay";
const ACP_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// Bound on each bootstrap request (initialize, session/new, session/load): an
// agent child that never replies must not pin the requesting blocking thread
// forever (issues/relay/26 defense-in-depth; startup's outer readiness poll
// gives up at 10s, but the inner request thread previously stayed stuck).
// Generous relative to observed bootstrap times; session/load streams its full
// replay before responding, so it shares the same wide bound.
const ACP_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

pub type DispatchHandler = Box<dyn FnOnce() + Send + 'static>;
// Permission handler is void-returning: the reader thread must not block on
// the operator's decision (see todos/acp/16). The handler receives a
// `PermissionResponder` it can move onto a separate task; that task awaits
// the decision and writes the JSON-RPC response via the responder when ready.
pub type PermissionHandler =
    Box<dyn FnMut(PermissionRequest, PermissionResponder) + Send + 'static>;
pub type PromptCompletionHandler = Box<dyn FnOnce(PromptCompletion) + Send + 'static>;

// Owns the obligation to write the agent's `session/request_permission`
// response. The handler installed by relay delivery moves the responder onto
// a short-lived resolver thread; once the operator's decision arrives, the
// resolver calls `respond` (or, if the resolver path drops it without
// responding, `Drop` emits a cancelled outcome) so the agent never waits
// forever on a permission it issued.
pub struct PermissionResponder {
    stdin: SharedStdin,
    request_id: u64,
    in_flight_flag: Arc<AtomicBool>,
    responded: bool,
}

impl PermissionResponder {
    pub fn respond(&mut self, decision: Option<String>) {
        if self.responded {
            return;
        }
        send_permission_response(&self.stdin, self.request_id, decision);
        self.responded = true;
    }
}

impl Drop for PermissionResponder {
    fn drop(&mut self) {
        if !self.responded {
            send_permission_response(&self.stdin, self.request_id, None);
        }
        self.in_flight_flag.store(false, Ordering::SeqCst);
    }
}

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
pub(crate) type SharedReplay = Arc<Mutex<Vec<ReplayEntry>>>;
type SharedPending = Arc<Mutex<HashMap<u64, mpsc::Sender<ResponseEnvelope>>>>;
// Shared pending-tool-call map. The reader thread owns the parser-side
// writes (recording buffer_position on `tool_call`, removing on
// `tool_call_update`); the prompt path needs read/write access too
// because prompt-path appends can trip the buffer cap and drain the
// front of the buffer -- without position maintenance, recorded
// positions would dangle and the next `tool_call_update` would either
// mutate the wrong Invocation or panic. The cap-maintain helper is
// idempotent on empty maps and is therefore safe to call from both
// paths.
pub(in crate::acp) type SharedPendingToolCalls = Arc<Mutex<HashMap<String, PendingToolCall>>>;

struct ActivePrompt {
    session_id: String,
    request_id: u64,
    on_permission_request: Mutex<Option<PermissionHandler>>,
    on_completion: Mutex<Option<PromptCompletionHandler>>,
    // Single-in-flight contract: at most one permission request may be
    // outstanding on the active prompt at a time. The reader sets the flag
    // when it dispatches a permission request to the handler; the
    // `PermissionResponder` clears it on drop (after responding). A second
    // request that arrives while the flag is set is dropped synchronously
    // with a cancelled response and an inscription. The contract keeps the
    // resolver-thread state (and `pending_permission_outcome`) at most
    // one-deep; upgrading to a queue is a separate slice if federation load
    // demands it.
    permission_in_flight: Arc<AtomicBool>,
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
    pending_tool_calls: SharedPendingToolCalls,
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

// Kernel returns ETXTBSY (os error 26) when a `Command::spawn` targets an
// executable the kernel has marked in-use by another process (e.g. a parallel
// cargo build rewriting `target/debug/agentmux` while a test holds it exec'd).
// We retry only when explicitly opted in via `retry_on_text_busy`; the relay
// worker path opts in because it runs under `cargo test` and is the path that
// flakes, while the interactive TUI spawn keeps single-shot semantics so real
// spawn failures surface immediately.
const ACP_SPAWN_TEXT_BUSY_RETRIES_MAXIMUM: u8 = 2;
const ACP_SPAWN_TEXT_BUSY_RETRY_DELAY: Duration = Duration::from_millis(50);

fn is_text_busy_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(26)
}

fn spawn_command(
    parts: &[&str],
    working_directory: &Path,
    environment: &[(String, String)],
    retry_on_text_busy: bool,
    retries_remaining: u8,
) -> Result<Child, String> {
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
    match command.spawn() {
        Ok(child) => Ok(child),
        Err(error) if retry_on_text_busy && retries_remaining > 0 && is_text_busy_error(&error) => {
            thread::sleep(ACP_SPAWN_TEXT_BUSY_RETRY_DELAY);
            spawn_command(
                parts,
                working_directory,
                environment,
                retry_on_text_busy,
                retries_remaining - 1,
            )
        }
        Err(source) => Err(format!("spawn ACP stdio command failed: {source}")),
    }
}

impl AcpStdioClient {
    // Spawn the ACP agent directly (no shell middleman). The command
    // template is split on whitespace. Environment variables are passed
    // explicitly via the `environment` parameter.
    //
    // `retry_on_text_busy` re-attempts the spawn up to
    // `ACP_SPAWN_TEXT_BUSY_RETRIES_MAXIMUM` times if the kernel returns
    // ETXTBSY. The retry window is short; sustained failures surface as a
    // normal spawn error so callers can decide whether to fall back.
    //
    // TODO: Consider shell-word parsing (e.g. shell-words crate) for
    //       templates containing metacharacters ($, |, &&, backticks).
    pub fn spawn(
        command_template: &str,
        working_directory: &Path,
        environment: &[(String, String)],
        retry_on_text_busy: bool,
    ) -> Result<Self, String> {
        let parts: Vec<&str> = command_template.split_whitespace().collect();
        if parts.is_empty() {
            return Err("ACP command template is empty".to_string());
        }
        let mut child = spawn_command(
            &parts,
            working_directory,
            environment,
            retry_on_text_busy,
            ACP_SPAWN_TEXT_BUSY_RETRIES_MAXIMUM,
        )?;
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
        let pending_tool_calls: SharedPendingToolCalls = Arc::new(Mutex::new(HashMap::new()));
        let pending_responses: SharedPending = Arc::new(Mutex::new(HashMap::new()));
        let active_prompt: SharedActivePrompt = Arc::new(Mutex::new(None));

        let reader_handle = spawn_reader_thread(
            BufReader::new(stdout),
            Arc::clone(&stdin),
            Arc::clone(&replay_buffer),
            Arc::clone(&pending_tool_calls),
            Arc::clone(&pending_responses),
            Arc::clone(&active_prompt),
        );

        Ok(Self {
            child,
            stdin,
            replay_buffer,
            pending_tool_calls,
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
            ACP_OPERATION_TIMEOUT,
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
                ACP_OPERATION_TIMEOUT,
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
            ACP_OPERATION_TIMEOUT,
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
            permission_in_flight: Arc::new(AtomicBool::new(false)),
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
        super::text::append_text_lines(prompt, &mut user_lines);
        if !user_lines.is_empty() {
            // Lock order matches the reader-thread dispatch path
            // (buffer first, then pending_tool_calls) so the two paths
            // cannot deadlock. Without holding both locks here, a
            // prompt-path append that tripped the cap could evict a
            // Pending Invocation whose `buffer_position` is still in
            // the reader's map; the next `tool_call_update` would then
            // mutate the wrong buffer entry or panic on out-of-bounds.
            let mut buffer = self.replay_buffer.lock().expect("replay_buffer mutex");
            let mut pending = self
                .pending_tool_calls
                .lock()
                .expect("pending_tool_calls mutex");
            super::replay::append_replay_entries(
                &mut buffer,
                &mut pending,
                vec![ReplayEntry::User {
                    lines: user_lines,
                    source: UserSource::PromptPath,
                }],
            );
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

    // Bounded, resumable wait for the most recent `prompt()` call to complete.
    //
    // Returns `true` once completion is observed -- the background reader fired
    // its `on_completion` handler (response arrived or transport closed), or the
    // synchronous dispatch path failed and cleared the active prompt -- both of
    // which drop the prompt's `_completion_signal` sender and surface here as
    // `Disconnected`. Also returns `true` immediately when no prompt has been
    // submitted since the last completed wait.
    //
    // Returns `false` if `timeout` elapsed with the prompt still in flight; the
    // pending receiver is retained so the caller can poll again (for example,
    // interleaving a `shutdown_requested()` check between polls). The per-target
    // worker uses this to serialize the single-flight ACP prompt invariant
    // without an unbounded `recv()` that could pin its blocking thread across
    // process shutdown (an agent whose turn never completes would otherwise
    // block clean teardown until SIGKILL).
    pub fn wait_for_prompt_complete(&self, timeout: Duration) -> bool {
        let mut guard = self
            .last_prompt_signal
            .lock()
            .expect("last_prompt_signal mutex");
        let Some(receiver) = guard.as_ref() else {
            return true;
        };
        match receiver.recv_timeout(timeout) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                *guard = None;
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
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
        timeout: Duration,
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
        let envelope = match rx.recv_timeout(timeout) {
            Ok(envelope) => envelope,
            Err(RecvTimeoutError::Timeout) => {
                self.pending_responses
                    .lock()
                    .expect("pending mutex")
                    .remove(&request_id);
                return Err(AcpRequestError::Timeout(timeout));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(AcpRequestError::ConnectionClosed {
                    reason: "ACP transport closed before response".to_string(),
                });
            }
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
    pending_tool_calls: SharedPendingToolCalls,
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
                &pending_tool_calls,
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
    pending_tool_calls: &SharedPendingToolCalls,
    pending_responses: &SharedPending,
    active_prompt: &SharedActivePrompt,
) {
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
                    dispatch_session_update(&decoded, replay_buffer, pending_tool_calls)
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
    pending_tool_calls: &SharedPendingToolCalls,
) {
    let params = decoded.get("params").unwrap_or(&Value::Null);
    if params.get("update").is_none_or(Value::is_null) {
        return;
    }
    // Lock order: buffer first, then pending. The prompt path follows the
    // same order so the two paths cannot deadlock against each other.
    let mut buffer = replay_buffer.lock().expect("replay_buffer mutex");
    let mut pending = pending_tool_calls.lock().expect("pending_tool_calls mutex");
    super::replay::parse_replay_entries_from_params(params, &mut pending, &mut buffer);
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

    // Single-in-flight contract (see ActivePrompt::permission_in_flight):
    // a second permission request landing while one is unresolved is dropped
    // here with a cancelled response. The agent receives a normal response
    // (so it does not stall), and an inscription records the drop for
    // diagnostics. The PermissionResponder for the in-flight request clears
    // the flag on drop, so this gate is purely a concurrency guard, not a
    // permanent block.
    if active
        .permission_in_flight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        emit_inscription(
            "acp.reader.permission_dropped_already_in_flight",
            &json!({"id": request_id}),
        );
        send_permission_response(stdin, request_id, None);
        return;
    }

    let request = build_permission_request_from_params(params, request_id);
    let responder = PermissionResponder {
        stdin: Arc::clone(stdin),
        request_id,
        in_flight_flag: Arc::clone(&active.permission_in_flight),
        responded: false,
    };

    // The handler is expected to move the responder onto a separate task
    // (typically a short-lived thread) so the reader returns to its
    // `read_line` loop immediately. If the handler is unset (e.g. relay
    // delivery did not register one), the responder is dropped here, which
    // synthesizes a cancelled response and clears the in-flight flag.
    let mut handler_slot = active
        .on_permission_request
        .lock()
        .expect("permission mutex");
    match handler_slot.as_mut() {
        Some(handler) => handler(request, responder),
        None => {
            emit_inscription(
                "acp.reader.permission_dropped_no_handler",
                &json!({"id": request_id}),
            );
            // Responder dropped: sends cancelled, clears flag.
            drop(responder);
        }
    }
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

impl Drop for AcpStdioClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}
