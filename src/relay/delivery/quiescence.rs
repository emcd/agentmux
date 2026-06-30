//! Relay-side tmux delivery scheduling config.
//!
//! The pane quiescence poll loop is pure tmux behavior and lives in
//! [`crate::tmux::transport`], run by the transport's internal delivery task
//! before each flush group; this module keeps the relay-owned scheduling config
//! the delivery handlers populate. [`QuiescenceOptions`] rides on the async
//! delivery task (constructed by the `send`/`raww` handlers) and is unpacked
//! into the `DeliveryEnvelope`'s `quiet_window` at the worker. The bounded
//! prime window and wedge detection live on the per-coder tmux config and are
//! threaded through by the dispatch worker via
//! [`crate::transports::DeliveryEnvelope::prime_timeout_ms`].
use std::time::Duration;

const QUIET_WINDOW_MS_DEFAULT: u64 = 750;

#[derive(Clone, Copy, Debug)]
pub(in crate::relay) struct QuiescenceOptions {
    pub quiet_window: Duration,
}

impl QuiescenceOptions {
    /// Async delivery is unbounded by design — bounded only by the relay
    /// lifetime via shutdown — so the prime window lives on the per-coder
    /// tmux config and is threaded onto the [`DeliveryEnvelope`](crate::transports::DeliveryEnvelope)
    /// by the dispatch worker.
    pub(in crate::relay) fn for_async(quiet_window_ms: Option<u64>) -> Self {
        Self {
            quiet_window: Duration::from_millis(
                quiet_window_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(QUIET_WINDOW_MS_DEFAULT),
            ),
        }
    }
}
