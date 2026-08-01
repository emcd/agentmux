//! Tmux quiescence probe and prompt-readiness matching.
//!
//! This module owns the [`PaneQuiescenceProbe`] trait (the transport-internal
//! seam the tmux delivery task uses to observe pane quiescence + prompt
//! readiness), the real tmux-backed implementation, the
//! [`TmuxAsWedgeProbe`](struct.TmuxAsWedgeProbe.html) adapter that exposes a
//! probe as the cross-transport [`WedgeProbe`], and the prompt-readiness
//! matching logic that classifies whether the inspected pane tail matches the
//! configured prompt regex + cursor column. [`PromptReadinessEvaluation`] is
//! the structured result the wait loop consumes.
//!
//! Public surface (re-exported from [`super`]):
//! - [`PromptReadinessEvaluation`] — the structured readiness classification
//!   returned by [`PaneQuiescenceProbe::next_evaluation`].
//! - [`PaneQuiescenceProbe`] — the test seam the external test surface in
//!   `tests/unit/tmux_transport.rs` injects scripted probes into.
//! - [`wait_for_quiescent_pane_three_state`] — the three-state classifier
//!   entry point (running / unresponsive / wedged) the delivery task calls.
//!
//! Crate-private surface:
//! - [`RealPaneQuiescenceProbe`] — the tmux-backed probe used at runtime.
//! - [`build_prompt_readiness_matcher`], [`prompt_readiness_matches`] — the
//!   prompt-readiness matching helpers.
//!
//! The cross-transport [`WedgeProbe`] and the dedup helper
//! `should_emit_prompt_mismatch` live in `src/transports/quiescence.rs` (see
//! commit `8e50657`, which lifted the wedge/prime state machine into the
//! shared module); this module only owns the tmux-specific seam.

use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use regex::Regex;

use crate::configuration::PromptReadinessTemplate;
use crate::runtime::signals::shutdown_requested;
use crate::transports::{
    DeliveryDiagnosticContext, DeliveryWaitError, QuiescenceBounds, WedgeObservation, WedgeProbe,
    wait_for_quiescent_three_state,
};

use super::pane::{
    capture_pane_snapshot, resolve_active_pane_target, resolve_cursor_column,
    resolve_window_activity_marker, sanitize_diagnostic_text,
};

const PROMPT_INSPECT_LINES_DEFAULT: usize = 3;
const PROMPT_INSPECT_LINES_MAX: usize = 40;

// ---------------------------------------------------------------------------
// Quiescence poll loop
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PromptReadinessMatcher {
    prompt_regex: Regex,
    inspect_lines: usize,
    input_idle_cursor_column: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptReadinessEvaluation {
    pub ready: bool,
    pub mismatch_reason: Option<String>,
    pub inspected_block: Option<String>,
    pub regex_matched: Option<bool>,
    pub expected_cursor_column: Option<usize>,
    pub observed_cursor_column: Option<usize>,
}

/// Transport-internal seam for the tmux quiescence wait.
///
/// The real implementation ([`RealPaneQuiescenceProbe`]) wraps tmux queries
/// against the active pane. Tests inject scripted probes that drive the
/// classifier deterministically — see the unit tests in
/// `tests/unit/tmux_transport.rs` for the probe classes Tmux can reach
/// (unresponsive, slow-prompt, normal). There is no wedged class: Tmux passes
/// `wedge_detection: false`, so the classifier cannot return that verdict here.
///
/// `pub` to support the external test surface; the trait is not part of
/// the public runtime API (no other code outside `src/tmux` consumes it).
pub trait PaneQuiescenceProbe: Send {
    /// Resolves the current prompt-readiness evaluation for the target pane.
    /// The wait loop calls this twice per quiescence check (with a
    /// `quiet_window` sleep between) and compares results.
    fn next_evaluation(&mut self) -> Result<PromptReadinessEvaluation, String>;

    /// Resolves the active pane target for the target session (e.g. `%0`).
    /// Used by the wait loop to record the pane on terminal outcomes and to
    /// thread through to the wedge inscription event.
    fn resolve_active_pane(&mut self) -> Result<String, String>;

    /// Resolves the pane's terminal-output-write marker (Tmux's
    /// `#{window_activity}`) as a `u64` epoch-seconds value. The
    /// `quiescence_classify_step` cross-transport classifier compares
    /// this field between two consecutive observations to detect
    /// whether bytes were written to the terminal during the
    /// `quiet_window` (a positive "output is flowing" signal); an
    /// advance suppresses the wedge / unresponsive / delivered
    /// classifications via the Busy pre-classification. Returns
    /// `Ok(Some(0))` (constant activity, no advance possible) when
    /// the format is unavailable on the running tmux version or
    /// when the marker is unparseable; the cross-transport
    /// classifier treats this as "no activity signal available,"
    /// falling back to pre-change behavior for that probe.
    fn last_window_activity_marker(&mut self) -> Result<Option<u64>, String>;

    /// Blocks until the pane shows a change (the next observation differs
    /// from the previous one) or the supplied `deadline` elapses. Returns
    /// `Ok(())` on observed change; `Err(DeliveryWaitError::Timeout)` on
    /// deadline elapsed with no change; `Err(DeliveryWaitError::Failed)`
    /// on probe errors. The wait loop passes a deadline derived from the
    /// per-coder `prime_timeout_ms` so the probe bounds its wait by the
    /// same prime window the loop tracks.
    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError>;
}

/// Real [`PaneQuiescenceProbe`] backed by tmux queries. Holds the socket path
/// and target session id used by every observation; the underlying tmux
/// queries are the same primitives the legacy wait loop called directly.
pub(crate) struct RealPaneQuiescenceProbe<'a> {
    tmux_socket: &'a Path,
    target_session: &'a str,
    matcher: Option<PromptReadinessMatcher>,
}

impl<'a> RealPaneQuiescenceProbe<'a> {
    pub(crate) fn new(
        tmux_socket: &'a Path,
        target_session: &'a str,
        prompt_readiness: Option<&PromptReadinessTemplate>,
    ) -> Result<Self, DeliveryWaitError> {
        let matcher = build_prompt_readiness_matcher(prompt_readiness)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        Ok(Self {
            tmux_socket,
            target_session,
            matcher,
        })
    }
}

impl PaneQuiescenceProbe for RealPaneQuiescenceProbe<'_> {
    fn next_evaluation(&mut self) -> Result<PromptReadinessEvaluation, String> {
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)?;
        let snapshot = capture_pane_snapshot(self.tmux_socket, &pane_target)?;
        prompt_readiness_matches(
            self.tmux_socket,
            pane_target.as_str(),
            snapshot.as_str(),
            self.matcher.as_ref(),
        )
    }

    fn resolve_active_pane(&mut self) -> Result<String, String> {
        resolve_active_pane_target(self.tmux_socket, self.target_session)
    }

    fn last_window_activity_marker(&mut self) -> Result<Option<u64>, String> {
        // Re-query tmux for `#{window_activity}` at observation
        // time. The existing `resolve_window_activity_marker`
        // returns `Ok(None)` when the format is unavailable on the
        // running tmux version; we surface that as `Some(0)` so
        // the cross-transport classifier's Busy pre-classification
        // is silently disabled (constant activity => no comparator
        // advance => Busy never fires), preserving pre-change
        // behavior for older tmux versions.
        //
        // The parsed `u64` is the project-defined surface — the
        // cross-transport classifier uses it as a monotonic
        // comparison value, not as a wall-clock timestamp. Tmux's
        // `#{window_activity}` returns seconds-since-epoch on
        // modern versions, which is naturally monotonic within a
        // session lifetime.
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)?;
        let marker = resolve_window_activity_marker(self.tmux_socket, &pane_target)?;
        let value = marker
            .as_deref()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        Ok(Some(value))
    }

    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError> {
        // Sleep in short slices, polling the activity marker and pane
        // target. Returns as soon as either changes (or the deadline
        // elapses).
        let pane_target = resolve_active_pane_target(self.tmux_socket, self.target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let mut last_activity = resolve_window_activity_marker(self.tmux_socket, &pane_target)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let mut last_snapshot = capture_pane_snapshot(self.tmux_socket, &pane_target)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        loop {
            if shutdown_requested() {
                return Err(DeliveryWaitError::Shutdown);
            }
            if Instant::now() >= deadline {
                return Err(DeliveryWaitError::Timeout {
                    timeout: deadline.saturating_duration_since(Instant::now()),
                    readiness_mismatch: false,
                    mismatch_reason: None,
                });
            }
            // Keep the slice short so shutdown_requested is observed promptly.
            thread::sleep(Duration::from_millis(50));
            let pane_target_now = resolve_active_pane_target(self.tmux_socket, self.target_session)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if pane_target_now != pane_target {
                return Ok(());
            }
            let activity_now = resolve_window_activity_marker(self.tmux_socket, &pane_target_now)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if activity_now != last_activity {
                return Ok(());
            }
            let snapshot_now = capture_pane_snapshot(self.tmux_socket, &pane_target_now)
                .map_err(|reason| DeliveryWaitError::Failed { reason })?;
            if snapshot_now != last_snapshot {
                return Ok(());
            }
            last_activity = activity_now;
            last_snapshot = snapshot_now;
        }
    }
}

/// Adapter that exposes a [`PaneQuiescenceProbe`] as the cross-transport
/// [`WedgeProbe`]. Constructed per quiescence iteration by
/// [`wait_for_quiescent_pane_three_state`]; holds a `&mut` borrow so it
/// does not own the underlying probe.
///
/// The adapter calls `next_evaluation()` exactly once per
/// [`observe`](WedgeProbe::observe) call. This keeps each
/// quiescence iteration to two `observe()` calls (= two
/// `next_evaluation()` roundtrips), matching the legacy
/// `wait_for_quiescent_pane_three_state` call frequency. Scripted test
/// probes with `abort_after_calls` thresholds trip at the iteration count
/// the test expects rather than at 4x that count.
///
/// Pane target resolution is delegated to the underlying probe (which
/// returns the active tmux pane id like `%0`) so the state machine can
/// thread it through to its diagnostic inscriptions
/// (`delivery_ready`, `delivery_prime_timeout`,
/// `delivery_readiness_timeout`, `delivery_prompt_mismatch`).
struct TmuxAsWedgeProbe<'a, P: PaneQuiescenceProbe> {
    inner: &'a mut P,
}

impl<'a, P: PaneQuiescenceProbe> TmuxAsWedgeProbe<'a, P> {
    fn new(inner: &'a mut P) -> Self {
        Self { inner }
    }
}

impl<'a, P: PaneQuiescenceProbe> WedgeProbe for TmuxAsWedgeProbe<'a, P> {
    fn observe(&mut self) -> Result<WedgeObservation, String> {
        let evaluation = self.inner.next_evaluation()?;
        let pane_target = self.inner.resolve_active_pane()?;
        let activity_generation = self.inner.last_window_activity_marker()?.unwrap_or(0);
        let mismatch = if evaluation.ready {
            None
        } else {
            Some(crate::transports::ReadinessMismatch {
                reason: evaluation.mismatch_reason.clone().unwrap_or_default(),
                regex_matched: evaluation.regex_matched,
                expected_cursor_column: evaluation
                    .expected_cursor_column
                    .and_then(|c| u16::try_from(c).ok()),
                observed_cursor_column: evaluation
                    .observed_cursor_column
                    .and_then(|c| u16::try_from(c).ok()),
            })
        };
        Ok(WedgeObservation {
            inspected_tail: evaluation.inspected_block.unwrap_or_default(),
            is_prompt_ready: evaluation.ready,
            pane_target: Some(pane_target),
            mismatch,
            activity_generation,
        })
    }

    fn wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError> {
        self.inner.wait_for_change(deadline)
    }
}

/// Drives the delivery classifier over a [`PaneQuiescenceProbe`]. `pub` to
/// support the external test surface in `tests/unit/tmux_transport.rs`; the
/// function is not part of the runtime API (callers reach it via
/// `flush_and_resolve`).
///
/// Tmux outcomes derived from pane content:
/// - `running` — output flowing or settled at prompt. Returns `Ok(pane)`.
/// - `unresponsive` — the opted-in prime window elapsed with no observable
///   change. Returns `Err(DeliveryWaitError::Timeout)`.
/// - readiness expiry — the flush group's readiness bound elapsed. Returns
///   `Err(DeliveryWaitError::ReadinessTimeout)` with a reason describing the
///   last observation.
///
/// Tmux does NOT classify `wedged`. Inferring a terminal failure from the
/// absence of change in rendered content cannot distinguish a hung coder from
/// a permission dialog awaiting an operator, a compose box holding typed input,
/// or a coder working without terminal output, so this transport passes
/// `wedge_detection: false` unconditionally and the readiness bound supplies
/// the termination the classifier used to.
///
/// This is a thin wrapper that constructs a [`TmuxAsWedgeProbe`] adapter
/// and delegates to the cross-transport
/// [`wait_for_quiescent_three_state`] in `src/transports/quiescence.rs`.
/// The `Result<String, DeliveryWaitError>` return type that callers and unit
/// tests rely on is preserved; the pane target in the `Ok` value comes from
/// the post-wait observation the state machine reports (which differs from the
/// pre-wait pane target when the active pane changed during the wait).
///
/// `bounds` carries the same semantics as in the underlying
/// [`wait_for_quiescent_three_state`] (see that function's docs).
pub fn wait_for_quiescent_pane_three_state<P: PaneQuiescenceProbe>(
    probe: &mut P,
    diagnostics: &DeliveryDiagnosticContext<'_>,
    bounds: &QuiescenceBounds,
) -> Result<String, DeliveryWaitError> {
    let mut adapter = TmuxAsWedgeProbe::new(probe);
    wait_for_quiescent_three_state(&mut adapter, diagnostics, bounds, false)
}

fn build_prompt_readiness_matcher(
    template: Option<&PromptReadinessTemplate>,
) -> Result<Option<PromptReadinessMatcher>, String> {
    let Some(template) = template else {
        return Ok(None);
    };

    let prompt_regex = Regex::new(template.prompt_regex.as_str())
        .map_err(|source| format!("invalid prompt_readiness.prompt_regex: {source}"))?;
    let inspect_lines = template
        .inspect_lines
        .unwrap_or(PROMPT_INSPECT_LINES_DEFAULT)
        .clamp(1, PROMPT_INSPECT_LINES_MAX);

    Ok(Some(PromptReadinessMatcher {
        prompt_regex,
        inspect_lines,
        input_idle_cursor_column: template.input_idle_cursor_column,
    }))
}

/// OpenCode-specific compose-region second gate. Returns true when
/// the inspected block has the adjacent OpenCode frame suffix and
/// any of the three rows immediately preceding its info row contains
/// compose text in the left-of-sidebar columns.
///
/// The OpenCode frame signature is an info row followed immediately by
/// a 20-or-more-character separator and a status row containing
/// `ctrl+p commands`. Claude/Codex/Gemini panes whose prompt regex also
/// matches but lack this suffix are returned false here, so their
/// readiness evaluation is unchanged.
///
/// `compose text` is `┃` + 2-99 non-newline whitespace chars +
/// non-whitespace (sidebar text has 100+ leading spaces, which is
/// above the threshold; empty rows have no content at all). The
/// boundary is 99 spaces inclusive on the compose side and 100 spaces
/// inclusive on the sidebar side, matching the prior
/// `[ \t]{100,}` regex contract (>=100 spaces = sidebar). The `2..100`
/// range is exclusive on the right (100 spaces = sidebar, not
/// compose).
///
/// **Supported layout: exactly three input rows.** The preserved api-aux and
/// editor idle captures from OpenCode 1.18.9 show sidebar runs of 148 and 152
/// spaces, with a three-row input box in both. A future OpenCode layout change
/// (e.g. input box collapsing to fewer rows, or a wider pane changing the
/// sidebar whitespace count) requires this implementation to be revisited.
fn compose_region_has_text(block: &str) -> bool {
    let lines: Vec<&str> = block.lines().collect();
    // Prefer the current footer when older OpenCode frames remain in the tail.
    let info_row_idx = lines.windows(3).rposition(|window| {
        let trimmed = window[0].trim_start();
        let Some(after_bar) = trimmed.strip_prefix('┃') else {
            return false;
        };
        if !after_bar
            .chars()
            .any(|character| !character.is_whitespace())
        {
            return false;
        }
        let separator = window[1].trim();
        let Some(separator) = separator.strip_prefix('╹') else {
            return false;
        };
        if separator.chars().count() < 20 || !separator.chars().all(|character| character == '▀')
        {
            return false;
        }
        window[2].chars().next().is_some_and(char::is_whitespace)
            && window[2].contains("ctrl+p commands")
    });

    let Some(info_idx) = info_row_idx else {
        return false;
    };

    // Inspect the three rows immediately above the info row. The
    // input box in OpenCode's supported idle layout is exactly three rows;
    // a future layout change requires this implementation to be revisited.
    let start = info_idx.saturating_sub(3);
    for line in lines.iter().take(info_idx).skip(start) {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('┃') {
            continue;
        }
        let after_b = trimmed.split_at('┃'.len_utf8()).1;
        let ws_count = after_b.chars().take_while(|c| c.is_whitespace()).count();
        // Compose text: 2..100 leading whitespace (2..=99 inclusive),
        // then a non-whitespace char. Sidebar (100+ spaces then
        // content) and empty rows fall outside this band. Boundary:
        // exactly 100 spaces = sidebar (matches the prior regex
        // contract), exactly 99 spaces = compose.
        if !(2..100).contains(&ws_count) {
            continue;
        }
        if after_b
            .chars()
            .nth(ws_count)
            .is_some_and(|c| !c.is_whitespace())
        {
            return true;
        }
    }

    false
}

fn prompt_readiness_matches(
    tmux_socket: &Path,
    pane_target: &str,
    snapshot: &str,
    matcher: Option<&PromptReadinessMatcher>,
) -> Result<PromptReadinessEvaluation, String> {
    let Some(matcher) = matcher else {
        return Ok(PromptReadinessEvaluation {
            ready: true,
            ..PromptReadinessEvaluation::default()
        });
    };

    let inspected = snapshot
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .take(matcher.inspect_lines)
        .collect::<Vec<_>>();
    if inspected.is_empty() {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(
                "inspected pane tail was empty after trimming trailing blank lines".to_string(),
            ),
            regex_matched: Some(false),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }
    let mut ordered = inspected;
    ordered.reverse();
    let block = ordered.join("\n");
    if !matcher.prompt_regex.is_match(block.as_str()) {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some("prompt regex did not match inspected pane tail".to_string()),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(false),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }

    // Code-side compose-region second gate. OpenCode-specific;
    // self-gates on the OpenCode frame signature (separator +
    // `ctrl+p commands` status row) inside the helper, so the gate
    // is a no-op for non-OpenCode panes. Closes the top/middle/bottom
    // input-box compose gap that the regex's `(?m)^` shape cannot
    // close on its own.
    if compose_region_has_text(&block) {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(
                "input box contains compose text in left-of-sidebar columns".to_string(),
            ),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }

    let Some(expected_cursor_column) = matcher.input_idle_cursor_column else {
        return Ok(PromptReadinessEvaluation {
            ready: true,
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            ..PromptReadinessEvaluation::default()
        });
    };
    let cursor_column = resolve_cursor_column(tmux_socket, pane_target)?;
    if cursor_column != expected_cursor_column {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(format!(
                "cursor column {} did not match required {}",
                cursor_column, expected_cursor_column
            )),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            expected_cursor_column: Some(expected_cursor_column),
            observed_cursor_column: Some(cursor_column),
            ..PromptReadinessEvaluation::default()
        });
    }

    Ok(PromptReadinessEvaluation {
        ready: true,
        inspected_block: Some(sanitize_diagnostic_text(&block)),
        regex_matched: Some(true),
        expected_cursor_column: Some(expected_cursor_column),
        observed_cursor_column: Some(cursor_column),
        ..PromptReadinessEvaluation::default()
    })
}

// One inline `#[cfg(test)] mod tests` is permitted by project
// policy for crate-private-by-design paths. The compose-region
// helper is a localized OpenCode-specific gate; testing it via
// `prompt_readiness_matches` (the production path) avoids test-only
// public re-exports. Fixture cases are loaded from the byte-for-byte
// copies of the preserved api-aux and editor idle captures from OpenCode
// 1.18.9 under `tests/unit/fixtures/opencode_idle_captures/`; the regex is
// read from the shipped `data/configuration/coders.toml` so the test cannot
// drift from production.
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const API_AUX_FIXTURE: &str = "api-aux-idle-1.18.9.txt";
    const EDITOR_FIXTURE: &str = "editor-idle-1.18.9.txt";

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("unit")
            .join("fixtures")
            .join("opencode_idle_captures")
            .join(name)
    }

    /// Read the OpenCode `prompt-readiness` block out of the shipped
    /// `data/configuration/coders.toml` directly, with a minimal text
    /// scan (the `toml` crate would pull a heavier dependency for
    /// one test). The expected shape inside the `[[coders]]` block
    /// with `id = 'opencode'` is `prompt-regex = '...'` plus
    /// optional `prompt-inspect-lines` and `prompt-idle-column`. We
    /// pull the regex string verbatim (TOML single-quoted) and the
    /// integer fields, if present.
    fn read_opencode_template() -> PromptReadinessTemplate {
        let raw = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data")
                .join("configuration")
                .join("coders.toml"),
        )
        .expect("read coders.toml");
        // Find the [[coders]] block whose `id = 'opencode'`.
        let mut in_opencode = false;
        let mut prompt_regex: Option<String> = None;
        let mut inspect_lines: Option<usize> = None;
        let mut input_idle_column: Option<usize> = None;
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("[[coders]]") {
                // New [[coders]] block: exit the previous one. Do
                // not reset the captured fields here; they are
                // overwritten by the opencode block's own fields if
                // the new block is opencode, and ignored otherwise.
                in_opencode = false;
                continue;
            }
            if trimmed.starts_with("id = 'opencode'") {
                in_opencode = true;
                continue;
            }
            if !in_opencode {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("prompt-regex = '") {
                let value = rest.strip_suffix('\'').unwrap_or(rest);
                prompt_regex = Some(value.to_string());
            } else if let Some(rest) = trimmed.strip_prefix("prompt-inspect-lines = ")
                && let Ok(n) = rest.parse::<usize>()
            {
                inspect_lines = Some(n);
            } else if let Some(rest) = trimmed.strip_prefix("prompt-idle-column = ")
                && let Ok(n) = rest.parse::<usize>()
            {
                input_idle_column = Some(n);
            }
        }
        PromptReadinessTemplate {
            prompt_regex: prompt_regex.expect("opencode prompt-regex not found in coders.toml"),
            inspect_lines,
            input_idle_cursor_column: input_idle_column,
        }
    }

    fn matcher_for_opencode() -> PromptReadinessMatcher {
        let mut template = read_opencode_template();
        // Disable the cursor-column check; the inline test is for
        // the regex + compose-region gate only. `resolve_cursor_column`
        // would otherwise invoke the real tmux binary, which is
        // unavailable in the test environment.
        template.input_idle_cursor_column = None;
        build_prompt_readiness_matcher(Some(&template))
            .expect("build_prompt_readiness_matcher")
            .expect("opencode coder should produce a matcher")
    }

    fn info_row_idx(snapshot: &str) -> usize {
        snapshot
            .lines()
            .collect::<Vec<_>>()
            .iter()
            .rposition(|l| {
                let t = l.trim_start();
                t.starts_with('┃') && t.contains(" · ")
            })
            .expect("info row not found")
    }

    /// Replace the input-box row at `position` (top/middle/bottom)
    /// with `┃  <text>` so the new row is compose text.
    fn mutate_input_row_with_text(snapshot: &str, position: &str, text: &str) -> String {
        let info = info_row_idx(snapshot);
        let row = match position {
            "top" => info.saturating_sub(3),
            "middle" => info.saturating_sub(2),
            "bottom" => info.saturating_sub(1),
            _ => panic!("position must be top|middle|bottom"),
        };
        let mut out: Vec<String> = snapshot.lines().map(String::from).collect();
        out[row] = format!("  ┃  {text}");
        out.join("\n")
    }

    /// Replace the input-box row at `position` with `┃` followed by
    /// `n` spaces then a non-whitespace char. Used for the
    /// 99/100/101 boundary tests.
    fn mutate_input_row_with_n_spaces(snapshot: &str, position: &str, n: usize) -> String {
        let info = info_row_idx(snapshot);
        let row = match position {
            "top" => info.saturating_sub(3),
            "middle" => info.saturating_sub(2),
            "bottom" => info.saturating_sub(1),
            _ => panic!("position must be top|middle|bottom"),
        };
        let mut out: Vec<String> = snapshot.lines().map(String::from).collect();
        out[row] = format!("  ┃{:width$}hello", "", width = n);
        out.join("\n")
    }

    #[test]
    fn prompt_readiness_path_opencode_compose_gate() {
        let matcher = matcher_for_opencode();

        // Read the two preserved OpenCode 1.18.9 idle captures.
        let api_aux =
            fs::read_to_string(fixture_path(API_AUX_FIXTURE)).expect("read api-aux fixture");
        let editor = fs::read_to_string(fixture_path(EDITOR_FIXTURE)).expect("read editor fixture");

        // Non-OpenCode synthetic block: different separator and
        // status-row token. The OpenCode regex must not match; the
        // compose check must not run (no-op on a non-matching block).
        let non_opencode = "\
  >>> some separator
  prompt > READY";
        let non_opencode_matcher = PromptReadinessMatcher {
            prompt_regex: Regex::new(r"(?s).*").expect("generic test regex"),
            inspect_lines: 10,
            input_idle_cursor_column: None,
        };
        let non_opencode_composing = "\
  ┃  typed text
  ┃
  ┃
  ┃  Ready";
        let malformed_opencode = "\
  ┃  typed text
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
  ┃ unrelated content
  status ctrl+p commands";
        let multi_frame_matcher = PromptReadinessMatcher {
            prompt_regex: Regex::new(r"(?s).*").expect("multi-frame test regex"),
            inspect_lines: 20,
            input_idle_cursor_column: None,
        };
        let older_composing_current_idle = "\
  ┃  old text
  ┃
  ┃
  ┃  Older
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /old ctrl+p commands
  ┃
  ┃
  ┃
  ┃  Current
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /current ctrl+p commands";
        let older_idle_current_composing = "\
  ┃
  ┃
  ┃
  ┃  Older
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /old ctrl+p commands
  ┃  current text
  ┃
  ┃
  ┃  Current
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /current ctrl+p commands";

        // ---- both preserved idle captures ready ----
        let eval =
            prompt_readiness_matches(Path::new("(test)"), "(test)", &api_aux, Some(&matcher))
                .expect("api-aux eval");
        assert_eq!(eval.regex_matched, Some(true), "api-aux regex must match");
        assert!(eval.ready, "api-aux idle must be ready (composes = empty)");

        let eval = prompt_readiness_matches(Path::new("(test)"), "(test)", &editor, Some(&matcher))
            .expect("editor eval");
        assert_eq!(eval.regex_matched, Some(true), "editor regex must match");
        assert!(eval.ready, "editor idle must be ready (composes = empty)");

        // ---- top/middle/bottom compose rejected ----
        for position in ["top", "middle", "bottom"] {
            let snap = mutate_input_row_with_text(&api_aux, position, "hello");
            let eval =
                prompt_readiness_matches(Path::new("(test)"), "(test)", &snap, Some(&matcher))
                    .expect("compose-text eval");
            assert!(
                !eval.ready,
                "compose text in {position} input row must be rejected"
            );
            assert_eq!(
                eval.regex_matched,
                Some(true),
                "regex still matches the mutated {position} frame"
            );
        }

        // ---- 99/100/101 boundary (bottom row; the same shape in
        // top/middle follows from the symmetric helper) ----
        for (n, expected_ready) in [(99usize, false), (100, true), (101, true)] {
            let snap = mutate_input_row_with_n_spaces(&api_aux, "bottom", n);
            let eval =
                prompt_readiness_matches(Path::new("(test)"), "(test)", &snap, Some(&matcher))
                    .expect("boundary eval");
            let label = match n {
                99 => "compose",
                100 => "sidebar",
                101 => "sidebar",
                _ => unreachable!(),
            };
            assert_eq!(
                eval.ready, expected_ready,
                "{n}-space row must be classified as {label}"
            );
        }

        // ---- non-OpenCode block unchanged (no compose check runs) ----
        let eval =
            prompt_readiness_matches(Path::new("(test)"), "(test)", non_opencode, Some(&matcher))
                .expect("non-opencode eval");
        assert_eq!(
            eval.regex_matched,
            Some(false),
            "non-OpenCode regex must not match"
        );
        assert!(
            !eval.ready,
            "non-OpenCode block must remain non-ready (regex mismatch)"
        );

        // A non-OpenCode matcher that matches a compose-like block
        // must remain ready because the OpenCode frame suffix is
        // absent. This exercises the production path after regex
        // matching, rather than the earlier mismatch branch.
        let eval = prompt_readiness_matches(
            Path::new("(test)"),
            "(test)",
            non_opencode_composing,
            Some(&non_opencode_matcher),
        )
        .expect("non-OpenCode matching eval");
        assert_eq!(eval.regex_matched, Some(true));
        assert!(
            eval.ready,
            "matching non-OpenCode compose-like block must remain ready"
        );

        // OpenCode-looking tokens without an adjacent suffix must
        // also bypass the compose predicate. Independent contains
        // checks would incorrectly reject this matching frame.
        let eval = prompt_readiness_matches(
            Path::new("(test)"),
            "(test)",
            malformed_opencode,
            Some(&non_opencode_matcher),
        )
        .expect("malformed OpenCode eval");
        assert_eq!(eval.regex_matched, Some(true));
        assert!(eval.ready, "non-adjacent frame tokens must remain ready");

        // The current frame is the bottommost valid suffix. Both
        // orderings prove stale frame state cannot control readiness.
        let eval = prompt_readiness_matches(
            Path::new("(test)"),
            "(test)",
            older_composing_current_idle,
            Some(&multi_frame_matcher),
        )
        .expect("older composing/current idle eval");
        assert!(
            eval.ready,
            "current idle frame must override older composing frame"
        );
        let eval = prompt_readiness_matches(
            Path::new("(test)"),
            "(test)",
            older_idle_current_composing,
            Some(&multi_frame_matcher),
        )
        .expect("older idle/current composing eval");
        assert!(
            !eval.ready,
            "current composing frame must override older idle frame"
        );

        // ---- teeth-check: with the compose-gate call removed from
        // prompt_readiness_matches, all four compose cases (top,
        // middle, bottom, 99-space) would turn ready. The asserts
        // above pin them as non-ready, so removing the call flips
        // them to failure. ----
    }
}
