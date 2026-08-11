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

use super::permissions::{PermissionHandler, PermissionResponder};
use super::{PROTOCOL_VERSION, PendingToolCall, ReplayEntry, UserSource};
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
    child: SharedChild,
    stdin: SharedStdin,
    replay_buffer: SharedReplay,
    pending_tool_calls: SharedPendingToolCalls,
    pending_responses: SharedPending,
    active_prompt: SharedActivePrompt,
    reader_handle: Option<JoinHandle<()>>,
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

/// The agent child, and the latch saying it has been told to end.
///
/// The two live together because the latch's whole purpose is to be honoured by
/// whoever holds the child: a termination request that arrives while the lock is
/// taken must not be dropped just because it could not be served at that instant.
#[derive(Debug)]
struct SupervisedChild {
    child: Mutex<Child>,
    /// Set once and never cleared. Read *after* every release of the child lock,
    /// so a request that lost a `try_lock` is served by whoever was holding it
    /// rather than lost.
    terminating: AtomicBool,
}

type SharedChild = Arc<SupervisedChild>;

impl SupervisedChild {
    /// Runs `action` against the child, then serves any termination request that
    /// arrived while the lock was held.
    ///
    /// Every lock of the child goes through here. Step 3 is contracted
    /// non-blocking, so it can only ever *try* for the lock, and something has to
    /// answer for the attempts that fail.
    ///
    /// The latch is read after the guard is dropped, and that ordering is the
    /// whole mechanism. Reading it while still holding the lock leaves a window
    /// nothing covers: the holder sees `false`, the request is latched, its
    /// `try_lock` fails against the holder that has already looked, and the
    /// holder then releases without looking again. Both sides have to be able to
    /// serve the request for either to be allowed to give up on it — hence: if
    /// the store lands before the release, this read sees it; if it lands after,
    /// the requester's own attempt finds the lock free.
    fn with_child<R>(&self, action: impl FnOnce(&mut Child) -> R) -> Option<R> {
        let outcome = {
            let mut child = self.child.lock().ok()?;
            action(&mut child)
        };
        if self.terminating.load(Ordering::Acquire) {
            self.initiate_termination();
        }
        Some(outcome)
    }

    /// Latches the termination request, then makes one non-blocking attempt to
    /// serve it. A failed attempt is not a failure: whoever holds the lock reads
    /// the latch as it releases and serves it there.
    ///
    /// Idempotent, so the release-point handoff can simply call it again.
    fn initiate_termination(&self) {
        self.terminating.store(true, Ordering::Release);
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.kill();
        }
    }
}

/// The fencing surface of one ACP generation, detached from the client that
/// owns it.
///
/// Exists because ownership and supervision diverge here: the client is moved
/// into the delivery thread it drives, so the transport that must terminate and
/// observe the generation no longer holds it. Everything the fence needs — the
/// child to signal, the reader's cessation to read — is shared state, so a
/// handle taken before the move keeps both steps reaching the real process.
#[derive(Clone, Debug)]
pub struct AcpGenerationHandle {
    child: SharedChild,
    reader_ceased: Arc<AtomicBool>,
}

impl AcpGenerationHandle {
    /// The fence's forced step: signals the child and returns. Killing it closes
    /// its stdio, which unblocks a reader parked in `read_line` and a writer
    /// parked on its stdin — the executors a cooperative flag cannot reach.
    ///
    /// Latches the request before attempting it, and never blocks. Taking the
    /// lock outright would have parked step 3 behind a teardown holding it
    /// across `wait`, which is exactly the blocking the step is contracted not
    /// to do.
    pub fn initiate_termination(&self) {
        self.child.initiate_termination();
    }

    /// Whether the reader thread of this generation has left its loop.
    #[must_use]
    pub fn reader_ceased(&self) -> bool {
        self.reader_ceased.load(Ordering::Acquire)
    }
}

/// Marks a reader thread ceased when it leaves, however it leaves.
///
/// A drop guard rather than a store at the end of the loop: a reader that
/// panicked is no longer executing either, and a fence that waited for it to
/// tidily set a flag would never go positive.
struct CeasedOnDrop(Arc<AtomicBool>);

impl Drop for CeasedOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) fn write_line_to_stdin(stdin: &SharedStdin, payload: &str) -> io::Result<()> {
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
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("ACP stdio child stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("ACP stdio child stdout unavailable"))?;

        let stdin = Arc::new(Mutex::new(stdin));
        let replay_buffer: SharedReplay = Arc::new(Mutex::new(Vec::new()));
        let pending_tool_calls: SharedPendingToolCalls = Arc::new(Mutex::new(HashMap::new()));
        let pending_responses: SharedPending = Arc::new(Mutex::new(HashMap::new()));
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
    pub fn note_prompt_dispatched(&self, prompt: &str, on_dispatched: Option<DispatchHandler>) {
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
    reader_ceased: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("acp-reader".to_string())
        .spawn(move || {
            let _ceased = CeasedOnDrop(reader_ceased);
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
            super::permissions::send_permission_response(stdin, request_id, None);
            return;
        }
    };

    if session_id.is_some_and(|sid| sid != active.session_id.as_str()) {
        emit_inscription(
            "acp.reader.permission_dropped_session_mismatch",
            &json!({"id": request_id}),
        );
        super::permissions::send_permission_response(stdin, request_id, None);
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
        super::permissions::send_permission_response(stdin, request_id, None);
        return;
    }

    let request = super::permissions::build_permission_request_from_params(params, request_id);
    let responder = PermissionResponder::new(
        Arc::clone(stdin),
        request_id,
        Arc::clone(&active.permission_in_flight),
    );

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

impl Drop for AcpStdioClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}
