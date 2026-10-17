//! `AcpStdioClient`: spawning the agent child, framing JSON-RPC over
//! stdio, and the prompt/request surface.
//!
//! Mechanical split of the former `client.rs`; no semantic changes.

use std::{
    collections::HashMap,
    io::{self, BufReader, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::AtomicBool,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::acp::permissions::PermissionHandler;
use crate::acp::replay::append_replay_entries;
use crate::acp::text::append_text_lines;
use crate::acp::{PROTOCOL_VERSION, ReplayEntry, UserSource};

use super::reader::spawn_reader_thread;
use super::types::{
    AcpGenerationHandle, AcpRequestError, ActivePrompt, DispatchHandler,
    PromptCompletionHandler, PromptDispatchOutcome, ResponseEnvelope,
    SharedActivePrompt, SharedChild, SharedPending, SharedPendingToolCalls,
    SharedReplay, SharedStdin, SupervisedChild,
};

const ACP_CLIENT_NAME: &str = "agentmux-relay";
const ACP_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// Bound on each bootstrap request (initialize, session/new, session/load): an
// agent child that never replies must not pin the requesting blocking thread
// forever (issues/relay/26 defense-in-depth; startup's outer readiness poll
// gives up at 10s, but the inner request thread previously stayed stuck).
// Generous relative to observed bootstrap times; session/load streams its full
// replay before responding, so it shares the same wide bound.
const ACP_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

pub struct AcpStdioClient {
    child: SharedChild,
    stdin: SharedStdin,
    replay_buffer: SharedReplay,
    pending_tool_calls: SharedPendingToolCalls,
    pending_responses: SharedPending,
    active_prompt: SharedActivePrompt,
    reader_handle: Option<thread::JoinHandle<()>>,
    /// Set when the reader thread leaves its loop, by normal exit or by panic.
    ///
    /// Shared rather than derived from `reader_handle` because the client is
    /// moved into the delivery thread that owns it, taking the handle out of
    /// reach of the transport that has to observe cessation. A flag survives the
    /// move; a `JoinHandle` does not.
    reader_ceased: Arc<AtomicBool>,
    next_id: u64,
    last_prompt_signal: Mutex<Option<mpsc::Receiver<()>>>,
}

pub(in crate::acp) fn write_line_to_stdin(
    stdin: &SharedStdin,
    payload: &str,
) -> io::Result<()> {
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
) -> io::Result<Child> {
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
        Err(error)
            if retry_on_text_busy
                && retries_remaining > 0
                && is_text_busy_error(&error) =>
        {
            thread::sleep(ACP_SPAWN_TEXT_BUSY_RETRY_DELAY);
            spawn_command(
                parts,
                working_directory,
                environment,
                retry_on_text_busy,
                retries_remaining - 1,
            )
        },
        Err(source) => Err(source),
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
    ) -> io::Result<Self> {
        let parts: Vec<&str> = command_template.split_whitespace().collect();
        if parts.is_empty() {
            return Err(io::Error::other("ACP command template is empty"));
        }
        let mut child = spawn_command(
            &parts,
            working_directory,
            environment,
            retry_on_text_busy,
            ACP_SPAWN_TEXT_BUSY_RETRIES_MAXIMUM,
        )?;
        let stdin = child.stdin.take().ok_or_else(|| {
            io::Error::other("ACP stdio child stdin unavailable")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::other("ACP stdio child stdout unavailable")
        })?;

        let stdin = Arc::new(Mutex::new(stdin));
        let replay_buffer: SharedReplay = Arc::new(Mutex::new(Vec::new()));
        let pending_tool_calls: SharedPendingToolCalls =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_responses: SharedPending =
            Arc::new(Mutex::new(HashMap::new()));
        let active_prompt: SharedActivePrompt = Arc::new(Mutex::new(None));

        let reader_ceased = Arc::new(AtomicBool::new(false));
        let reader_handle = spawn_reader_thread(
            BufReader::new(stdout),
            Arc::clone(&stdin),
            Arc::clone(&replay_buffer),
            Arc::clone(&pending_tool_calls),
            Arc::clone(&pending_responses),
            Arc::clone(&active_prompt),
            Arc::clone(&reader_ceased),
        );

        Ok(Self {
            child: Arc::new(SupervisedChild {
                child: Mutex::new(child),
                terminating: AtomicBool::new(false),
            }),
            stdin,
            replay_buffer,
            pending_tool_calls,
            pending_responses,
            active_prompt,
            reader_handle: Some(reader_handle),
            reader_ceased,
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
                format!(
                    "ACP initialize timed out after {}ms",
                    timeout.as_millis()
                )
            },
            AcpRequestError::ConnectionClosed { reason } => reason,
            AcpRequestError::TransportUnavailable { reason } => reason,
        })
    }

    pub fn new_session(
        &mut self,
        working_directory: &Path,
    ) -> Result<String, String> {
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
                    format!(
                        "ACP session/new timed out after {}ms",
                        timeout.as_millis()
                    )
                },
                AcpRequestError::ConnectionClosed { reason } => reason,
                AcpRequestError::TransportUnavailable { reason } => reason,
            })?;
        result
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                "ACP session/new response missing result.sessionId".to_string()
            })
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
                format!(
                    "ACP session/load timed out after {}ms",
                    timeout.as_millis()
                )
            },
            AcpRequestError::ConnectionClosed { reason } => reason,
            AcpRequestError::TransportUnavailable { reason } => reason,
        })?;
        let buffer = self.replay_buffer.lock().expect("replay_buffer mutex");
        Ok(buffer[entries_before_load..].to_vec())
    }

    /// Dispatches a `session/prompt` request. The framed write is the delivery
    /// boundary: `Submitted` is returned immediately after the write succeeds,
    /// before the replay-buffer locks or the dispatch callback run. The caller
    /// records the member's submission evidence on `Submitted`, then invokes
    /// [`Self::note_prompt_dispatched`] to append the prompt-path replay entry
    /// and fire the dispatch callback. Active-prompt refusal and serialization
    /// failure return `SerializationFailed`; a stdin write or flush error
    /// returns `TransportUnavailable`.
    pub fn prompt(
        &mut self,
        session_id: &str,
        prompt: &str,
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
            let mut slot =
                self.active_prompt.lock().expect("active_prompt mutex");
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
                *self.active_prompt.lock().expect("active_prompt mutex") =
                    None;
                return PromptDispatchOutcome::SerializationFailed(format!(
                    "serialize ACP prompt failed: {source}"
                ));
            },
        };

        if let Err(source) = write_line_to_stdin(&self.stdin, message.as_str())
        {
            *self.active_prompt.lock().expect("active_prompt mutex") = None;
            return PromptDispatchOutcome::TransportUnavailable {
                reason: format!("write ACP prompt failed: {source}"),
            };
        }

        // The framed `session/prompt` write is the submission boundary: `Submitted`
        // is recorded immediately after the write succeeds, before the
        // replay-buffer locks or `on_dispatched` run. Either of those can block or
        // panic and would otherwise strand evidence that had already been earned.
        // The caller invokes [`Self::note_prompt_dispatched`] after recording the
        // member's evidence.
        PromptDispatchOutcome::Submitted
    }

    /// Records the prompt-path replay entries and fires the dispatch callback
    /// after a `Submitted` outcome from [`Self::prompt`]. Kept separate so the
    /// framed write's evidence is recorded before the replay-buffer locks or
    /// `on_dispatched` — neither of which may interpose between the write and the
    /// evidence that proves it happened.
    pub fn note_prompt_dispatched(
        &self,
        prompt: &str,
        on_dispatched: Option<DispatchHandler>,
    ) {
        let mut user_lines: Vec<String> = Vec::new();
        append_text_lines(prompt, &mut user_lines);
        if !user_lines.is_empty() {
            // Lock order matches the reader-thread dispatch path
            // (buffer first, then pending_tool_calls) so the two paths
            // cannot deadlock. Without holding both locks here, a
            // prompt-path append that tripped the cap could evict a
            // Pending Invocation whose `buffer_position` is still in
            // the reader's map; the next `tool_call_update` would then
            // mutate the wrong buffer entry or panic on out-of-bounds.
            let mut buffer =
                self.replay_buffer.lock().expect("replay_buffer mutex");
            let mut pending = self
                .pending_tool_calls
                .lock()
                .expect("pending_tool_calls mutex");
            append_replay_entries(
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
            },
            Err(RecvTimeoutError::Timeout) => false,
        }
    }

    pub fn replay_entries_since(
        &self,
        cursor: usize,
    ) -> (Vec<ReplayEntry>, usize) {
        let buffer = self.replay_buffer.lock().expect("replay_buffer mutex");
        let len = buffer.len();
        if cursor >= len {
            return (Vec::new(), len);
        }
        (buffer[cursor..].to_vec(), len)
    }

    pub fn child_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.with_child(|child| child.stderr.take())?
    }

    /// Hands out this generation's fencing surface, so a supervisor can keep
    /// terminating and observing it after the client is moved into the delivery
    /// thread it drives.
    #[must_use]
    pub fn generation_handle(&self) -> AcpGenerationHandle {
        AcpGenerationHandle {
            child: Arc::clone(&self.child),
            reader_ceased: Arc::clone(&self.reader_ceased),
        }
    }

    pub fn shutdown(&mut self) {
        // Killing the child closes its stdout, which makes the reader's
        // blocking `read_line` return EOF (Ok(0)) and exit the loop. The
        // reader then drops its clones of the shared stdin, replay buffer,
        // and pending-response registry; pending senders dropped in the
        // registry signal `Disconnected` to any waiters in `prompt`/`request`.
        self.child.with_child(|child| {
            let _ = child.kill();
            let _ = child.wait();
        });
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
            let mut pending =
                self.pending_responses.lock().expect("pending mutex");
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
            AcpRequestError::Failed(format!(
                "serialize ACP request failed: {source}"
            ))
        })?;
        if let Err(source) = write_line_to_stdin(&self.stdin, message.as_str())
        {
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
            },
            Err(RecvTimeoutError::Disconnected) => {
                return Err(AcpRequestError::ConnectionClosed {
                    reason: "ACP transport closed before response".to_string(),
                });
            },
        };
        match envelope {
            ResponseEnvelope::Result(value) => Ok(value),
            ResponseEnvelope::Error(reason) => {
                Err(AcpRequestError::Failed(reason))
            },
        }
    }
}

impl Drop for AcpStdioClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}
