//! What a transport reports about itself, and how the `look` path windows it.
//!
//! [`TransportStatus`] and [`TransportReadiness`] answer a startup call;
//! [`TransportError`] is the structured failure a transport returns rather than
//! a bare string; [`LookMode`] carries the relay's look-surface policy into
//! [`OutputView::look`](super::OutputView::look).

use std::time::Duration;

use serde_json::Value;

/// The result of a [`Transport::startup`](super::Transport::startup) call.
#[derive(Clone, Debug)]
pub struct TransportStatus {
    pub readiness: TransportReadiness,
}

/// Readiness of a transport runtime after startup.
#[derive(Clone, Debug)]
pub enum TransportReadiness {
    /// Ready to accept delivery immediately.
    Ready,
    /// Established but not yet ready (for example, awaiting first prompt).
    Pending,
    /// Could not be established; carries the failure taxonomy.
    Unavailable { code: String, reason: String },
}

/// A structured transport failure surfaced to the relay worker.
#[derive(Clone, Debug)]
pub struct TransportError {
    pub code: String,
    pub reason: String,
    pub details: Option<Value>,
}

/// Windowing parameters for an [`OutputView::look`](super::OutputView::look)
/// snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct LookMode {
    /// Window size (tmux pane lines or ACP replay entries).
    pub lines: Option<u64>,
    /// Entries to skip from the newest end before the tail window (ACP only).
    pub offset: Option<u64>,
    /// How long the handle may wait for a still-initializing target to populate
    /// its first snapshot before returning a stale-tagged result. The relay
    /// supplies this as its look-surface policy; a zero duration means no wait.
    pub prime_timeout: Duration,
}
