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
    /// Retained until the shared configuration cleanup removes these inputs.
    pub prime_timeout_ms: Option<u64>,
    pub wedge_detection: bool,
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

impl OutputView for PtyOutputView {
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.shared
            .snapshot_tx
            .blocking_send(SnapshotRequest {
                inspect_lines: mode.lines.map(|lines| lines as usize),
                tx,
            })
            .map_err(|_request| TransportError {
                code: "internal_unexpected_failure".to_string(),
                reason: "pty snapshot channel closed; transport not running".to_string(),
                details: None,
            })?;
        let response = rx.blocking_recv().map_err(|_canceled| TransportError {
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

/// Pty prompt-readiness observer used by the handover predicate.
pub struct PtyPromptProbe {
    shared: PtyShared,
}

impl PtyPromptProbe {
    #[must_use]
    pub fn new(shared: PtyShared) -> Self {
        Self { shared }
    }

    pub fn observe(&mut self) -> Result<bool, String> {
        let (tx, rx) = oneshot::channel();
        self.shared
            .snapshot_tx
            .blocking_send(SnapshotRequest {
                inspect_lines: Some(usize::from(self.shared.config.prompt_inspect_lines)),
                tx,
            })
            .map_err(|_request| "pty snapshot channel closed; transport not running".to_string())?;
        let response = rx
            .blocking_recv()
            .map_err(|_canceled| "pty snapshot response canceled before delivery".to_string())?;
        let regex_matches = self
            .shared
            .config
            .prompt_regex
            .as_ref()
            .is_none_or(|regex| regex.is_match(&response.tail));
        let cursor_ready = self
            .shared
            .config
            .prompt_idle_column
            .is_none_or(|expected| response.cursor_x == expected);
        Ok(regex_matches && cursor_ready)
    }
}

impl PtyConfigSnapshot {
    /// Stable target identifier used by transport diagnostics and tests.
    pub fn target_id(&self) -> &str {
        &self.target_member_id
    }
}
