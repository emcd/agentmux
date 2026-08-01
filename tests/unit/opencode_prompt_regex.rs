//! Compact table-driven coverage for the opencode prompt-readiness regex.
//!
//! The regex is shipped in `data/configuration/coders.toml` under the
//! `opencode` coder's `prompt-regex` field. It matches the Opencode
//! prompt UI frame structure AND requires the LAST 1-3 rows before the
//! info row to be empty `┃` or sidebar-only `┃` (100+ spaces before
//! content).
//!
//! ## What it checks
//!
//! Two gates per inspected block:
//! 1. The Opencode prompt UI is visible -- a `┃` line, then the
//!    separator `╹▀▀▀...`, then the status row that ends with
//!    `ctrl+p commands` (with optional `• OpenCode <version>` tail).
//!    The info row anchor is mode-agnostic (`┃\s+\S[^\n]*`, not
//!    `┃\s+Build · [^\n]*`): earlier forms that hardcoded the `Build`
//!    label regressed the moment Opencode switched to a different
//!    label (e.g. `Plan · <model>`), because the row reads
//!    `<label> · <model>` and the label is not a stable invariant.
//! 2. The LAST 1-3 rows before the info row are empty or sidebar-only.
//!    Two explicit input-box-row shapes: empty `┃\s*$`, or sidebar-only
//!    `┃[ \t]{100,}\S[^\n]*` (100+ leading spaces before content).
//!    Rust's stable `regex` crate does not support look-around, so the
//!    "compose text excluded" check is expressed as a positive
//!    alternation. The 100-space lower bound is well under the
//!    shortest measured sidebar run (148 spaces in api-aux idle,
//    152 in editor idle); text with 34-99 leading spaces is correctly
//!    rejected as compose text rather than classified as sidebar.
//!    The `{1,3}` window covers Opencode's variable input-box height
//!    (it can collapse to 1 row during/after agent processing).
//!
//! ## What it does NOT check -- the partial-protection ceiling
//!
//! The regex anchors on the row immediately before the info row. It
//! does NOT examine rows above the `{1,3}` window, so a compose with
//! text only in the TOP row of the input box (with the bottom rows
//! empty and the cursor parked at col 5 of the bottom row) still
//! matches. Verified: typing into the top of the input box in
//! `backend/.auxiliary/scribbles/relay-58-pane-captures/` produces
//! match=true, while typing into the middle or bottom produces
//! match=false. So this protects the bottom input row but not the
//! middle or top.
//!
//! This is a structural ceiling rather than a missing trick. "Every
//! row of a variable-height box is empty" is not expressible as a
//! single unanchored regex without look-around, and Rust's stable
//! `regex` crate has none. Three iterations have now tried to encode
//! that structural property in a pattern language that cannot state
//! it; each has traded one failure mode for another. Remaining gap
//! will need a different mechanism (e.g. libghostty-vt for screen
//! state parsing, or a multi-pass code-side check), not a fourth
//! regex draft. Recorded as a real narrowing relative to `3fe6fb8`;
//! the cursor-column check remains the only universal guard.
//!
//! The cursor-column check is wired in
//! `src/tmux/quiescence_probe.rs` via `prompt-idle-column = 5` and runs
//! alongside the regex. For Opencode versions whose idle cursor column
//! is not 5 (e.g. an empirical fresh-session probe on 1.18.10 saw
//! the idle cursor at col 16; production bundles confirmed on 1.18.5
//! and 1.18.9 see col 5), the field needs updating separately. The
//! regex here is cursor-column-INDEPENDENT.

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
  ┃                                                                                                                                                                                          ~/src/WORKTREES/agentmux/api-aux:api-
  ┃  Build · GPT-5.6 Sol OpenAI                                                        aux
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
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
// idle column. The cursor-column check passes (col 5); the regex still
// rejects because the bottom input row contains compose text. The
// gap c145365 left open and this option (b) closes -- for the BOTTOM
// row of the input box.
const HOME_AFTER_TYPING: &str = "\
  ┃
  ┃
  ┃  hello
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

// User has typed text only in the TOP row of the input box; the
// middle and bottom rows are empty (cursor at col 5 of the bottom
// row, typical Home-after-typing position). The regex's `{1,3}`
// window can start at the empty bottom row and matches the frame.
// This is the structural-ceiling narrowing documented in the module
// doc: the regex protects the bottom row, not the middle or top.
// Recorded here as match=true to keep the partial-protection
// behaviour honest; the cursor-column check is the only universal
// guard for this case.
const COMPOSING_TOP: &str = "\
  ┃  hello
  ┃
  ┃
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

// Opencode may switch the info-row label from "Build" to another
// agent/mode label (e.g. "Plan" for plan-mode agents). The info-row
// anchor is mode-agnostic (`┃\s+\S[^\n]*`, not `┃\s+Build · `) so
// the regex matches any such label. This fixture protects against a
// regression to the label-hardcoded form that would stop all
// delivery on label change.
const PLAN_MODE: &str = "\
  ┃
  ┃
  ┃
  ┃  Plan · o3-mini · 12.3s
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/plan                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

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
        ("plan_mode", PLAN_MODE, true),
        ("composing", COMPOSING, false),
        ("composing_full", COMPOSING_FULL, false),
        ("composing_top", COMPOSING_TOP, true),
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
