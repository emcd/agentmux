//! Shared types for the ACP stdio client: outcome enums, shared-state
//! aliases, child supervision, and the generation fencing handle.
//!
//! Mechanical split of the former `client.rs`; no semantic changes.

use std::{
    collections::HashMap,
    process::{Child, ChildStdin},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use serde_json::Value;

use super::super::{PendingToolCall, ReplayEntry};
use crate::acp::permissions::PermissionHandler;

pub type DispatchHandler = Box<dyn FnOnce() + Send + 'static>;
pub type PromptCompletionHandler =
    Box<dyn FnOnce(PromptCompletion) + Send + 'static>;

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
pub(super) enum ResponseEnvelope {
    Result(Value),
    Error(String),
}

pub(in crate::acp) type SharedStdin = Arc<Mutex<ChildStdin>>;
pub(crate) type SharedReplay = Arc<Mutex<Vec<ReplayEntry>>>;
pub(super) type SharedPending =
    Arc<Mutex<HashMap<u64, mpsc::Sender<ResponseEnvelope>>>>;
// Shared pending-tool-call map. The reader thread owns the parser-side
// writes (recording buffer_position on `tool_call`, removing on
// `tool_call_update`); the prompt path needs read/write access too
// because prompt-path appends can trip the buffer cap and drain the
// front of the buffer -- without position maintenance, recorded
// positions would dangle and the next `tool_call_update` would either
// mutate the wrong Invocation or panic. The cap-maintain helper is
// idempotent on empty maps and is therefore safe to call from both
// paths.
pub(in crate::acp) type SharedPendingToolCalls =
    Arc<Mutex<HashMap<String, PendingToolCall>>>;

pub(super) struct ActivePrompt {
    pub(super) session_id: String,
    pub(super) request_id: u64,
    pub(super) on_permission_request: Mutex<Option<PermissionHandler>>,
    pub(super) on_completion: Mutex<Option<PromptCompletionHandler>>,
    // Single-in-flight contract: at most one permission request may be
    // outstanding on the active prompt at a time. The reader sets the flag
    // when it dispatches a permission request to the handler; the
    // `PermissionResponder` clears it on drop (after responding). A second
    // request that arrives while the flag is set is dropped synchronously
    // with a cancelled response and an inscription. The contract keeps the
    // resolver-thread state (and `pending_permission_outcome`) at most
    // one-deep; upgrading to a queue is a separate slice if federation load
    // demands it.
    pub(super) permission_in_flight: Arc<AtomicBool>,
    // Dropped when the active_prompt slot is cleared (after on_completion
    // fires, or on synchronous dispatch failure). The matching receiver
    // on `AcpStdioClient.last_prompt_signal` then observes `Disconnected`
    // and `wait_for_prompt_complete()` returns. Never `.send()`'d; presence
    // of the sender is itself the "still in flight" signal.
    pub(super) _completion_signal: mpsc::Sender<()>,
}

pub(super) type SharedActivePrompt = Arc<Mutex<Option<Arc<ActivePrompt>>>>;

/// The agent child, and the latch saying it has been told to end.
///
/// The two live together because the latch's whole purpose is to be honoured by
/// whoever holds the child: a termination request that arrives while the lock is
/// taken must not be dropped just because it could not be served at that instant.
#[derive(Debug)]
pub(super) struct SupervisedChild {
    pub(super) child: Mutex<Child>,
    /// Set once and never cleared. Read *after* every release of the child lock,
    /// so a request that lost a `try_lock` is served by whoever was holding it
    /// rather than lost.
    pub(super) terminating: AtomicBool,
}

pub(super) type SharedChild = Arc<SupervisedChild>;

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
    pub(super) fn with_child<R>(
        &self,
        action: impl FnOnce(&mut Child) -> R,
    ) -> Option<R> {
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
    pub(super) fn initiate_termination(&self) {
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
    pub(super) child: SharedChild,
    pub(super) reader_ceased: Arc<AtomicBool>,
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
pub(super) struct CeasedOnDrop(pub(super) Arc<AtomicBool>);

impl Drop for CeasedOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
