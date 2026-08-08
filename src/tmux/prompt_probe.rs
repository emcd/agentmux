//! Tmux pane prompt-readiness observation.
//!
//! The transport reads this level before authorization. Delivery itself does
//! not wait for a pane to become quiet or prompt-ready; that would make a
//! target-side observation part of the write outcome rather than admission.

use std::path::Path;

use regex::Regex;

use super::pane::{
    capture_pane_snapshot, resolve_active_pane_target, resolve_cursor_column,
    sanitize_diagnostic_text,
};
use crate::configuration::PromptReadinessTemplate;

const PROMPT_INSPECT_LINES_DEFAULT: usize = 3;
const PROMPT_INSPECT_LINES_MAX: usize = 40;

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

/// Transport-internal seam for Tmux prompt-readiness observation.
///
/// The real implementation (`RealPanePromptProbe`) wraps tmux queries against
/// the active pane. Tests inject scripted probes through this seam.
pub trait PanePromptProbe: Send {
    /// Resolves the current prompt-readiness evaluation for the target pane.
    fn next_evaluation(&mut self) -> Result<PromptReadinessEvaluation, String>;
}

/// Real [`PanePromptProbe`] backed by tmux queries.
pub(crate) struct RealPanePromptProbe<'a> {
    tmux_socket: &'a Path,
    target_session: &'a str,
    matcher: Option<PromptReadinessMatcher>,
}

impl<'a> RealPanePromptProbe<'a> {
    pub(crate) fn new(
        tmux_socket: &'a Path,
        target_session: &'a str,
        prompt_readiness: Option<&PromptReadinessTemplate>,
    ) -> Result<Self, String> {
        let matcher = build_prompt_readiness_matcher(prompt_readiness)?;
        Ok(Self {
            tmux_socket,
            target_session,
            matcher,
        })
    }
}

impl PanePromptProbe for RealPanePromptProbe<'_> {
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
