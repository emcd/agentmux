//! Pty transport state and shared view.
//!
//! [`PtyState`] holds the libghostty-vt terminal, response buffer for the
//! `on_pty_write` effect handler, and the snapshot-request receiver the
//! delivery thread services. The terminal is `!Send + !Sync`, so
//! [`PtyState`] stays on the worker thread and the cross-thread
//! coordination goes through a `mpsc::Sender`/`mpsc::Receiver`
//! snapshot-request channel (not via `Arc<Mutex<Terminal>>`, which
//! would not be `Send` and therefore could not be shared across the
//! relay look thread and the delivery thread).
//!
//! [`PtyShared`] is the `Send + Sync` wrapper the look path and
//! [`PtyQuiescenceProbe`] consume. It carries the per-coder config
//! snapshot, the last-byte timestamp used by the `is_settled` check,
//! and the snapshot-request sender.
//!
//! [`PtyOutputView`] is the [`OutputView`](crate::transports::OutputView)
//! handle the relay's look request path reads without borrowing the
//! worker-owned transport. It captures a screen snapshot by sending a
//! snapshot request through the channel; the worker renders the snapshot
//! from the live terminal and replies via the oneshot.
//!
//! [`PtyQuiescenceProbe`] adapts [`PtyShared`] to the cross-transport
//! [`WedgeProbe`](crate::transports::WedgeProbe) trait so the shared
//! state machine in [`crate::transports::quiescence`] drives Pty's
//! quiescence wait using the same wedge/prime-timeout semantics as
//! Tmux. The probe computes `is_prompt_ready` from the snapshot's
//! regex match + cursor position (per-coder config from
//! [`PtyShared::config`]) and exposes the readiness mismatch metadata
//! for the state machine's diagnostic inscriptions.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use tokio::sync::{mpsc, oneshot};

use crate::transports::{
    DeliveryWaitError, LookMode, LookSnapshotPayload, OutputView, ReadinessMismatch,
    TransportError, WedgeObservation, WedgeProbe,
};

/// Default pty look window applied when the caller omits a window size.
/// Mirrors the Tmux transport's `LOOK_LINES_DEFAULT`.
pub const LOOK_LINES_DEFAULT: usize = 120;

/// Per-coder pty config snapshot carried on the `Send + Sync` shared
/// state. Lives on [`PtyShared`] so the look path and the
/// [`PtyQuiescenceProbe`] can consult it without borrowing the worker.
#[derive(Clone, Debug)]
pub struct PtyConfigSnapshot {
    /// Target session id this transport is bound to.
    pub target_member_id: String,
    /// Initial grid columns (matches the per-coder config; Pty spawns
    /// the child at these dims).
    pub cols: u16,
    /// Initial grid rows.
    pub rows: u16,
    /// Optional prompt-readiness regex (the `prompt_regex` field in
    /// `[coders.<id>.pty]`). When set, the probe applies it to the
    /// snapshot tail to determine `is_prompt_ready`.
    pub prompt_regex: Option<Regex>,
    /// Number of trailing rows to format for prompt-readiness checks.
    /// Default 3, clamped to 1..=40 by the config validator.
    pub prompt_inspect_lines: u16,
    /// When set, the probe requires the cursor to idle at this column
    /// to consider the target prompt-ready.
    pub prompt_idle_column: Option<u16>,
    /// Bounded prime window (mirrors `[coders.<id>.pty].prime-timeout-ms`).
    pub prime_timeout_ms: Option<u64>,
    /// Whether wedge detection is enabled (mirrors
    /// `[coders.<id>.pty].wedge-detection`; default true).
    pub wedge_detection: bool,
}

/// One snapshot request routed from the look path or the quiescence
/// probe through the snapshot channel to the worker thread. The worker
/// renders the requested tail from the live terminal and replies via
/// the oneshot.
pub struct SnapshotRequest {
    /// How many trailing rows to format. `None` uses
    /// [`LOOK_LINES_DEFAULT`].
    pub inspect_lines: Option<usize>,
    /// Oneshot channel for the worker's reply. The worker sends the
    /// snapshot back via this sender; the look / probe caller awaits
    /// via the receiver.
    pub tx: oneshot::Sender<SnapshotResponse>,
}

/// The worker's reply to a [`SnapshotRequest`]. Carries the rendered
/// tail plus cursor position + visibility, which is everything the look
/// path and the [`PtyQuiescenceProbe`] need to do their work without
/// holding a reference to the terminal.
#[derive(Clone, Debug)]
pub struct SnapshotResponse {
    /// The rendered tail (the last `inspect_lines` rows formatted as
    /// text via `Formatter::format_alloc(Format::Plain)` with `trim`
    /// enabled). Empty when the formatted screen is whitespace-only.
    pub tail: String,
    /// Cursor column at the time of observation (`Terminal::cursor_x()`).
    pub cursor_x: u16,
    /// Cursor row at the time of observation (`Terminal::cursor_y()`).
    pub cursor_y: u16,
    /// Whether the cursor is visible at the time of observation.
    pub cursor_visible: bool,
}

/// `Send + Sync` shared state consulted by the look path and the
/// [`PtyQuiescenceProbe`]. Holds the per-coder config snapshot, the
/// last-byte atomic timestamp (used by the probe's `wait_for_change`),
/// and the snapshot-request sender (the receiver lives in [`PtyState`]
/// on the worker thread).
#[derive(Clone)]
pub struct PtyShared {
    pub config: PtyConfigSnapshot,
    pub last_byte_atomic: Arc<AtomicU64>,
    pub snapshot_tx: mpsc::Sender<SnapshotRequest>,
}

/// Worker-thread-local state for a single Pty target. Holds the
/// libghostty-vt terminal (which is `!Send + !Sync`) and the
/// snapshot-request receiver the worker services from the same
/// thread.
///
/// Not `Send` by design — the terminal must live on a single thread.
/// The transport owns this state on the worker thread; the cross-thread
/// coordination goes through [`PtyShared`] (snapshot channel +
/// `last_byte_atomic`).
///
/// Effect-handler state (the `on_pty_write` response buffer, etc.) lives
/// in the writer Arc the handler closure captures at startup; the
/// handler writes responses back to the PTY master synchronously inside
/// `vt_write`. There is no shared buffer between the handler and the
/// worker.
pub struct PtyState {
    /// The libghostty-vt terminal. `!Send + !Sync`. Owned exclusively
    /// by the worker thread that drives the PTY.
    pub terminal: libghostty_vt::Terminal<'static, 'static>,
    /// Snapshot-request channel receiver. The worker selects on this
    /// in its main loop alongside the bytes / write-command channels.
    pub snapshot_rx: mpsc::Receiver<SnapshotRequest>,
}

/// Handle the relay's look request path reads without borrowing the
/// worker-owned transport. See [`crate::transports::OutputView`].
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
        let request = SnapshotRequest {
            inspect_lines: mode.lines.map(|lines| lines as usize),
            tx,
        };
        // The worker services snapshot requests in its main loop; if
        // the channel is full or closed, surface a transport error so
        // the relay can decide how to recover (rather than silently
        // returning empty data).
        self.shared
            .snapshot_tx
            .blocking_send(request)
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
        let tail = response.tail;
        let requested_lines = mode
            .lines
            .map(|lines| lines as usize)
            .unwrap_or(LOOK_LINES_DEFAULT);
        let snapshot_lines: Vec<String> = tail
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

/// Cross-transport [`WedgeProbe`] adapter for Pty. Computes
/// `is_prompt_ready` from the snapshot's regex match + cursor position
/// (per-coder config from [`PtyConfigSnapshot`]) and exposes the
/// readiness mismatch metadata for the state machine's diagnostic
/// inscriptions.
pub struct PtyQuiescenceProbe {
    shared: PtyShared,
}

impl PtyQuiescenceProbe {
    #[must_use]
    pub fn new(shared: PtyShared) -> Self {
        Self { shared }
    }
}

// `BundleMember` is referenced via the `target_member_id` field on
// `PtyConfigSnapshot`; the full bundle-member binding (initial command,
// resume command, working directory) lands in §6 alongside the
// `[coders.<id>.pty]` config parser.
#[allow(unused_imports)]
use crate::configuration::BundleMember as _ReexportBundleMember;

impl WedgeProbe for PtyQuiescenceProbe {
    fn observe(&mut self) -> Result<WedgeObservation, String> {
        let (tx, rx) = oneshot::channel();
        let request = SnapshotRequest {
            inspect_lines: Some(usize::from(self.shared.config.prompt_inspect_lines)),
            tx,
        };
        self.shared
            .snapshot_tx
            .blocking_send(request)
            .map_err(|_request| "pty snapshot channel closed; transport not running".to_string())?;
        let response = rx
            .blocking_recv()
            .map_err(|_canceled| "pty snapshot response canceled before delivery".to_string())?;

        // Determine readiness from the snapshot's tail + cursor
        // position against the per-coder prompt regex + idle column.
        let regex_matches = match self.shared.config.prompt_regex.as_ref() {
            Some(regex) => regex.is_match(&response.tail),
            None => true, // No regex configured → vacuously ready.
        };
        let cursor_idle_at_expected = match self.shared.config.prompt_idle_column {
            Some(expected) => response.cursor_x == expected,
            None => true, // No idle column configured → vacuously ready.
        };
        let is_prompt_ready = regex_matches && cursor_idle_at_expected;

        let mismatch = if is_prompt_ready {
            None
        } else {
            let reason = if !regex_matches && self.shared.config.prompt_regex.is_some() {
                "prompt regex did not match inspected pane tail".to_string()
            } else if !cursor_idle_at_expected {
                format!(
                    "cursor column {} did not match required {}",
                    response.cursor_x,
                    self.shared.config.prompt_idle_column.map_or(0, |c| c)
                )
            } else {
                String::new()
            };
            Some(ReadinessMismatch {
                reason,
                regex_matched: Some(regex_matches),
                expected_cursor_column: self.shared.config.prompt_idle_column,
                observed_cursor_column: Some(response.cursor_x),
            })
        };

        Ok(WedgeObservation {
            inspected_tail: response.tail,
            is_prompt_ready,
            operator_interaction_active: false,
            pane_target: None,
            mismatch,
        })
    }

    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError> {
        // Poll the last-byte atomic timestamp. Pty's reader thread
        // updates it on every byte it forwards into the terminal;
        // a change indicates new terminal output is available, which
        // breaks quiescence.
        let initial = self.shared.last_byte_atomic.load(Ordering::Acquire);
        while Instant::now() < deadline {
            if self.shared.last_byte_atomic.load(Ordering::Acquire) != initial {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(DeliveryWaitError::Timeout {
            timeout: deadline.saturating_duration_since(Instant::now()),
            readiness_mismatch: false,
            mismatch_reason: None,
        })
    }
}

#[cfg(test)]
mod tests {}
