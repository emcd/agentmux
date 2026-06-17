//! Relay-side tmux delivery scheduling config.
//!
//! The pane quiescence poll loop is pure tmux behavior and lives in
//! [`crate::tmux::transport`]; this module keeps the relay-owned scheduling
//! config the delivery handlers populate. [`QuiescenceOptions`] rides on the
//! async delivery task (constructed by the `send`/`raww` handlers) and is
//! unpacked into primitives at the tmux hoist boundary, so the loop never
//! depends on relay.

use std::time::Duration;

const QUIET_WINDOW_MS_DEFAULT: u64 = 750;
// Also caps the UI-reconnect delivery wait when the caller set no explicit
// quiescence timeout (see `deliver_one_target_ui`).
pub(super) const QUIESCENCE_TIMEOUT_MS_DEFAULT: u64 = 30_000;

#[derive(Clone, Copy, Debug)]
pub(in crate::relay) struct QuiescenceOptions {
    pub quiet_window: Duration,
    pub quiescence_timeout: Option<Duration>,
}

impl Default for QuiescenceOptions {
    fn default() -> Self {
        Self {
            quiet_window: Duration::from_millis(QUIET_WINDOW_MS_DEFAULT),
            quiescence_timeout: Some(Duration::from_millis(QUIESCENCE_TIMEOUT_MS_DEFAULT)),
        }
    }
}

impl QuiescenceOptions {
    /// Async delivery is unbounded by design — bounded only by the relay
    /// lifetime via shutdown — so `quiescence_timeout` is always `None` here.
    pub(in crate::relay) fn for_async(quiet_window_ms: Option<u64>) -> Self {
        Self {
            quiet_window: Duration::from_millis(
                quiet_window_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(QUIET_WINDOW_MS_DEFAULT),
            ),
            quiescence_timeout: None,
        }
    }
}
