//! Compact table-driven coverage for the opencode prompt-readiness regex.
//!
//! The regex is shipped in `data/configuration/coders.toml` under the
//! `opencode` coder's `prompt-regex` field. It matches the Opencode
//! prompt UI frame structure AND requires the LAST 1-3 rows before the
//! info row to have no compose text in the left-of-sidebar columns.
//!
//! ## What it checks
//!
//! Two gates per inspected block:
//! 1. The Opencode prompt UI is visible -- a `┃` line, then the
//!    separator `╹▀▀▀...`, then the status row that ends with
//!    `ctrl+p commands` (with optional `• OpenCode <version>` tail).
//! 2. The LAST 1-3 rows before the info row have no compose text in the
//!    columns just to the right of the `┃` border. The compose-text
//!    pattern is `┃[^\S\n]{2,30}\S`: ┃ followed by 2-30 non-newline
//!    whitespace chars then a non-whitespace char. The class
//!    `[^\S\n]` (not `\s`) keeps the match within a single row, which
//!    is the bug a previous draft (`\s`) had -- `\s` matches `\n`,
//!    so the pattern crossed row boundaries on api-aux and flagged
//!    every idle pane as having pending text. The `{2,30}` upper
//!    bound is well under the minimum sidebar whitespace run measured
//!    in the api-aux idle capture (148 spaces), so sidebar text is
//!    not falsely flagged as compose text. The `{1,3}` window covers
//!    Opencode's variable input-box height (it can collapse to 1 row
//!    during/after agent processing).
//!
//! ## What it does NOT check
//!
//! - The cursor-column check is still wired in
//!   `src/tmux/quiescence_probe.rs` via `prompt-idle-column = 5` and
//!   runs alongside the regex. For Opencode versions whose idle cursor
//!   column is not 5, that field needs updating. The cursor check is
//!   exercised by `tests/integration/relay_delivery_prompt.rs`.
//! - Multi-line compose where only the TOP row of the input box has
//!   text and the bottom row is empty (with cursor parked at col 5 of
//!   the bottom row): the regex's `{1,3}` window can start at the
//!   empty bottom row and match. This is a documented narrowing
//!   relative to `3fe6fb8`'s strict "blank above info row" anchor;
//!   if it becomes reachable in practice (empirical probe pending),
//!   raise the window to `{3,3}`.
//!
//! ## Why this shape and not "frame structure only"
//!
//! A frame-structure-only match (c145365) leaves the cursor-column check
//! as the sole guard against delivery during composing. That is enough
//! for typical composing, but a residual gap remains: when the user has
//! typed text and the cursor has been navigated back to col 5 (Home /
//! Ctrl-A after typing, or shift+enter multi-line compose), both gates
//! pass and delivery lands on top of the user's pending input. The
//! column-bounded check above closes that gap by rejecting the input
//! frame when the input-box rows have compose text.

use std::fs;

use regex::Regex;
use toml::Value;

const IDLE_EDITOR: &str = "\
  ┃ Completely agree with you. The format of what landed was fine; the problem is that it should not have landed.
  ┃
  ┃
  ┃
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)                                                                                                ~/src/WORKTREES/agentmux/editor:editor
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /home/me/src/WORKTREES/agentmux/editor                                                                              97.9K (10%)  ctrl+p commands    • OpenCode 1.18.5";

const IDLE_ACP: &str = "\
  ┃
  ┃
  ┃
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /home/me/src/WORKTREES/agentmux/acp     35.4K (4%) · $0.15  ctrl+p commands    • OpenCode 1.18.5";

const COLD_START: &str = "\
  ┃
  ┃
  ┃
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            133.6K (13%) · $0.27 ctrl+p commands    • OpenCode 1.18.4";

const API_AUX_SIDEBAR_WRAP: &str = "\
  ┃
  ┃
  ┃                                                                                    ~/src/WORKTREES/agentmux/api-aux:api-
  ┃  Build · GPT-5.6 Sol OpenAI                                                        aux
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /home/me/src/WORKTREES/agentmux/api-aux                                                                            105.1K (21%)  ctrl+p commands    • OpenCode 1.18.9";

const AGENT_JUST_RESPONDED: &str = "\
  ┃ user prompt
  ┃  agent response text
  ┃
  ┃
  ┃
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            133.6K (13%) · $0.27 ctrl+p commands    • OpenCode 1.18.4";

// User has typed text in the bottom row of the input box. Opencode
// format is `┃  text` (two spaces between the `┃` border and the
// content). The column-bounded compose check sees this and rejects
// the frame.
const COMPOSING: &str = "\
  ┃
  ┃
  ┃  hello
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

// User has filled all three input rows. Both row counts and the
// compose-text pattern match the column-bounded reject.
const COMPOSING_FULL: &str = "\
  ┃  line 2
  ┃  line 1
  ┃  line 0
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

// User typed text, then pressed Home so the cursor sits back at the
// idle column (5). The cursor-column check passes (col 5); the
// column-bounded check still rejects because the input row contains
// compose text. This is the residual gap that c145365 left open and
// this option (b) closes.
const HOME_AFTER_TYPING: &str = "\
  ┃
  ┃
  ┃  hello
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

const BUSY_RETRY_BANNER: &str = "\
Our servers are currently overloaded. Please try again later.
[retrying in 5m 27s attempt #9]        esc interrupt    • OpenCode 1.18.9";

const EMPTY_PANE: &str = "\
some unrelated content
more content
";

fn read_opencode_prompt_regex_from_config() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let config_path = std::path::Path::new(manifest_dir)
        .join("data")
        .join("configuration")
        .join("coders.toml");
    let raw = fs::read_to_string(&config_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", config_path.display()));
    let value = raw
        .parse::<Value>()
        .unwrap_or_else(|error| panic!("parse {}: {error}", config_path.display()));
    let coders = value
        .get("coders")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{} has no `[[coders]]` table", config_path.display()));
    let opencode = coders
        .iter()
        .find(|coder| coder.get("id").and_then(Value::as_str) == Some("opencode"))
        .unwrap_or_else(|| panic!("opencode coder entry missing in {}", config_path.display()));
    opencode
        .get("tmux")
        .and_then(|tmux| tmux.get("prompt-regex"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "opencode coder entry missing tmux.prompt-regex in {}",
                config_path.display()
            )
        })
        .to_string()
}

#[test]
fn opencode_prompt_regex_classifies_states() {
    let regex = Regex::new(&read_opencode_prompt_regex_from_config())
        .expect("opencode prompt-regex from coders.toml must compile");

    let cases: &[(&str, &str, bool)] = &[
        ("idle_editor", IDLE_EDITOR, true),
        ("idle_acp", IDLE_ACP, true),
        ("cold_start", COLD_START, true),
        ("agent_just_responded", AGENT_JUST_RESPONDED, true),
        ("api_aux_sidebar_wrap", API_AUX_SIDEBAR_WRAP, true),
        ("composing", COMPOSING, false),
        ("composing_full", COMPOSING_FULL, false),
        ("home_after_typing", HOME_AFTER_TYPING, false),
        ("busy_retry_banner", BUSY_RETRY_BANNER, false),
        ("empty_pane", EMPTY_PANE, false),
    ];

    for (label, fixture, expected_match) in cases {
        let actual = regex.is_match(fixture);
        assert_eq!(
            actual, *expected_match,
            "fixture {label}: expected match={expected_match}, got match={actual}",
        );
    }
}
