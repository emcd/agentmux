//! Tmux output observation — `TmuxOutputView` and pane probe.
//!
//! Extracted from `transport/mod.rs` as a mechanical split — no behavior
//! change.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::transports::{LookMode, LookSnapshotPayload, TransportError};

use crate::tmux::pane::{capture_pane_tail_lines, resolve_active_pane_target};
use crate::tmux::prompt_probe::{PanePromptProbe, RealPanePromptProbe};

const LOOK_LINES_DEFAULT: usize = 120;

/// A config-constructed [`crate::transports::OutputView`] over a tmux session's active pane.
///
/// Unlike the ACP view, this holds no worker-owned state: it captures the tmux
/// pane directly through the socket, so it is valid before any delivery has
/// spawned a worker for the session. The relay's `get_output_view` accessor
/// constructs it from the socket path and session id.
pub struct TmuxOutputView {
    socket_path: PathBuf,
    session_id: String,
}

impl TmuxOutputView {
    /// Builds a view over the active pane of `session_id` on `socket_path`.
    #[must_use]
    pub fn new(socket_path: PathBuf, session_id: String) -> Self {
        Self {
            socket_path,
            session_id,
        }
    }
}

impl crate::transports::OutputView for TmuxOutputView {
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        if mode.offset.unwrap_or(0) > 0 {
            return Err(TransportError {
                code: "validation_offset_unsupported".to_string(),
                reason: "offset is only supported for ACP look targets".to_string(),
                details: Some(json!({ "offset": mode.offset })),
            });
        }
        let requested_lines = mode
            .lines
            .map(|lines| lines as usize)
            .unwrap_or(LOOK_LINES_DEFAULT);
        let pane_target =
            resolve_active_pane_target(self.socket_path.as_path(), self.session_id.as_str())
                .map_err(|reason| TransportError {
                    code: "internal_unexpected_failure".to_string(),
                    reason: "failed to resolve active pane for look target".to_string(),
                    details: Some(json!({ "cause": reason })),
                })?;
        let snapshot_lines = capture_pane_tail_lines(
            self.socket_path.as_path(),
            pane_target.as_str(),
            requested_lines,
        )
        .map_err(|reason| TransportError {
            code: "internal_unexpected_failure".to_string(),
            reason: "failed to capture look snapshot".to_string(),
            details: Some(json!({ "cause": reason })),
        })?;
        Ok(LookSnapshotPayload::Lines { snapshot_lines })
    }
}

/// One pane observation, separating "could not observe" from "observed".
///
/// `None` means the pane could not be inspected at all, which is what a departed
/// session or a dead tmux server looks like. `Some(ready)` means it was
/// inspected and this is what it said — a settled non-prompt frame is a *busy*
/// target and belongs to the readiness axis, not the health one.
///
/// A free function because the observer thread owns no transport: it holds only
/// what it needs to look, which is also what keeps it off the transport lock.
pub(super) fn observe_pane_once(
    socket: &Path,
    target_session: &str,
    prompt_readiness: Option<&crate::configuration::PromptReadinessTemplate>,
) -> Option<(bool, u64)> {
    let mut probe = RealPanePromptProbe::new(socket, target_session, prompt_readiness).ok()?;
    probe
        .next_evaluation()
        .ok()
        .map(|evaluation| (evaluation.ready, evaluation.activity_generation))
}
