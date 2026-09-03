//! Pty transport state and shared view.
//!
//! The terminal remains owned by the worker thread because libghostty-vt is
//! `!Send + !Sync`. Cross-thread look and readiness observations use snapshot
//! requests routed through that worker.

use std::sync::{Arc, atomic::AtomicBool};

use regex::Regex;
use tokio::sync::{mpsc, oneshot};

use crate::transports::{LookMode, LookSnapshotPayload, OutputView, TransportError};

/// Default pty look window applied when the caller omits a window size.
pub const LOOK_LINES_DEFAULT: usize = 120;

/// Per-coder pty configuration shared with the snapshot observer.
#[derive(Clone, Debug)]
pub struct PtyConfigSnapshot {
    pub target_member_id: String,
    pub cols: u16,
    pub rows: u16,
    pub prompt_regex: Option<Regex>,
    pub prompt_inspect_lines: u16,
    pub prompt_idle_column: Option<u16>,
}

/// Snapshot request routed from the look path or readiness observer through
/// the worker thread.
pub struct SnapshotRequest {
    pub inspect_lines: Option<usize>,
    pub tx: oneshot::Sender<SnapshotResponse>,
}

/// Worker response containing the rendered tail and cursor state.
#[derive(Clone, Debug)]
pub struct SnapshotResponse {
    pub tail: String,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub cursor_visible: bool,
}

/// `Send + Sync` state consulted by look and readiness paths.
#[derive(Clone)]
pub struct PtyShared {
    pub config: PtyConfigSnapshot,
    pub snapshot_tx: mpsc::Sender<SnapshotRequest>,
    pub child_exited: Arc<AtomicBool>,
}

/// Worker-thread-local terminal and snapshot receiver.
pub struct PtyState {
    pub terminal: libghostty_vt::Terminal<'static, 'static>,
    pub snapshot_rx: mpsc::Receiver<SnapshotRequest>,
}

/// Handle used by the relay's look request path.
pub struct PtyOutputView {
    shared: PtyShared,
}

impl PtyOutputView {
    #[must_use]
    pub fn new(shared: PtyShared) -> Self {
        Self { shared }
    }
}

impl PtyOutputView {
    /// Async handshake for the look path. The snapshot request is sent via
    /// `mpsc::Sender::send().await` and the response awaited, so the calling
    /// tokio worker thread is never blocked. This is the off-runtime path the
    /// transport-contracts spec requires.
    pub async fn look_async(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.shared
            .snapshot_tx
            .send(SnapshotRequest {
                inspect_lines: mode.lines.map(|lines| lines as usize),
                tx,
            })
            .await
            .map_err(|_request| TransportError {
                code: "internal_unexpected_failure".to_string(),
                reason: "pty snapshot channel closed; transport not running".to_string(),
                details: None,
            })?;
        let response = rx.await.map_err(|_canceled| TransportError {
            code: "internal_unexpected_failure".to_string(),
            reason: "pty snapshot response canceled before delivery".to_string(),
            details: None,
        })?;
        let requested_lines = mode
            .lines
            .map(|lines| lines as usize)
            .unwrap_or(LOOK_LINES_DEFAULT);
        let snapshot_lines = response
            .tail
            .lines()
            .rev()
            .take(requested_lines)
            .map(str::to_string)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Ok(LookSnapshotPayload::Lines { snapshot_lines })
    }
}

impl OutputView for PtyOutputView {
    fn look(&self, _mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        // `PtyOutputView::look` is currently unreachable via the relay's look
        // handler — `get_output_view` at `registry.rs:582` returns `None` for
        // `TargetConfiguration::Pty` unconditionally, so `look.rs:320` never
        // reaches this. The snapshot handshake itself is correct in
        // `look_async` (no `blocking_*` on a runtime thread); this sync
        // `OutputView::look` is intentionally unavailable until the registry
        // arm is scoped (follow-up). Returning a structured error is smaller
        // and more honest than a three-branch runtime-flavor dispatcher with a
        // panic handler for a path nothing calls.
        Err(TransportError {
            code: "internal_unexpected_failure".to_string(),
            reason: "pty look not yet available via relay registry; worker exists but get_output_view returns None until registry arm is scoped (follow-up)".to_string(),
            details: None,
        })
    }
}

/// Whether a rendered terminal snapshot satisfies this target's prompt-readiness
/// template.
///
/// Both halves are permissive when unconfigured, and deliberately so: a target
/// with no template configured is one the operator has said nothing about, and
/// withholding delivery from it would turn silence into a refusal.
///
/// Kept here rather than inside the delivery writer because it is the predicate,
/// not the observation. The writer reads the terminal it owns directly; a
/// cross-thread caller would have to go through the snapshot channel. Separating
/// the two is what lets the predicate be exercised against a snapshot the test
/// wrote itself, without a live terminal behind it.
#[must_use]
pub fn prompt_satisfied(config: &PtyConfigSnapshot, snapshot: &SnapshotResponse) -> bool {
    let regex_matches = config
        .prompt_regex
        .as_ref()
        .is_none_or(|regex| regex.is_match(&snapshot.tail));
    let cursor_ready = config
        .prompt_idle_column
        .is_none_or(|expected| snapshot.cursor_x == expected);
    regex_matches && cursor_ready
}

impl PtyConfigSnapshot {
    /// Stable target identifier used by transport diagnostics and tests.
    pub fn target_id(&self) -> &str {
        &self.target_member_id
    }
}
