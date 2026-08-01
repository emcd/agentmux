//! Compact table-driven coverage for the opencode prompt-readiness regex.
//!
//! The regex is shipped in `data/configuration/coders.toml` under the
//! `opencode` coder's `prompt-regex` field. It matches the Opencode
//! prompt UI frame structure: a `┃` line, then the separator
//! `╹▀▀▀...`, then the status row that ends with `ctrl+p commands`
//! (with optional `• OpenCode <version>` tail).
//!
//! ## Two-stage readiness gate
//!
//! 1. **Regex (this file's primary test)**: matches the Opencode
//!    prompt UI frame structure. Mode-agnostic info row anchor
//!    (`.*┃.*\n`) so label changes (e.g. `Plan · <model>`) don't
//!    regress; 20+ `▀` chars in the separator so the rendered run
//!    matches regardless of pane width; `ctrl+p commands` token in
//!    the status row so the version is recognised as Opencode.
//! 2. **Code-side compose-region check** (`compose_region_has_text`,
//!    re-exported from `src/tmux/quiescence_probe.rs`): walks up to
//!    three rows immediately preceding the info row and rejects
//!    compose text (`┃` + 2-100 leading whitespace + non-whitespace)
//!    in any of them. Closes the top/middle-row compose gap that
//!    the regex's `(?m)^`-anchored shape cannot express on its own
//!    (Rust's stable `regex` crate has no look-around; "every row of
//!    a variable-height box is empty" is not expressible as a single
//!    unanchored regex).
//!
//! The cursor-column check (`prompt-idle-column`) is wired alongside
//! both in `prompt_readiness_matches`; it remains the only universal
//! guard against `Home` / `Ctrl-A` after typing (cursor back at idle
//! column with text still in the input row).

use std::fs;

use regex::Regex;
use toml::Value;

const IDLE_EDITOR: &str = "\
  ┃ Completely agree with you. The format of what landed was fine; the problem is that it should not have landed.
  ┃
  ┃
  ┃
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)                                                                                                ~/src/WORKTREES/agentmux/editor:editor
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
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

// Measured sidebar run is 148 spaces (`agentmux:artifacts/10`). The
// synthetic fixture uses 150 spaces; the threshold-100 code-side
// check classifies it as sidebar-only (well above 100).
const API_AUX_SIDEBAR_WRAP: &str = "\
  ┃
  ┃
  ┃                                                                                                                              ~/src/WORKTREES/agentmux/api-aux:api-
  ┃  Build · GPT-5.6 Sol OpenAI                                                        aux
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
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

// Composing cases: text in input box. The regex matches the frame
// structure; the code-side `compose_region_has_text` rejects these.
const COMPOSING: &str = "\
  ┃
  ┃
  ┃  hello
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

const COMPOSING_FULL: &str = "\
  ┃  line 2
  ┃  line 1
  ┃  line 0
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

// User typed text only in the TOP row of the input box; middle and
// bottom are empty. The regex matches the frame; the code-side
// compose-region check rejects because the top row has compose text.
const COMPOSING_TOP: &str = "\
  ┃  hello
  ┃
  ┃
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

// User typed text only in the MIDDLE row; top and bottom are empty.
// Same as COMPOSING_TOP but text in the middle row.
const COMPOSING_MIDDLE: &str = "\
  ┃
  ┃  hello
  ┃
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

// User typed text then pressed Home so the cursor sits back at the
// idle column. The cursor-column check passes (col 5); the regex
// matches the frame; the compose-region check rejects because the
// bottom input row contains compose text.
const HOME_AFTER_TYPING: &str = "\
  ┃
  ┃
  ┃  hello
  ┃  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

// Opencode may switch the info-row label from "Build" to another
// agent/mode label (e.g. "Plan" for plan-mode agents). The info-row
// anchor `.*┃.*` is mode-agnostic; this fixture protects against a
// regression to a label-hardcoded form that would stop all delivery
// on label change.
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

    // Frame-structure only. Idle and composing fixtures both match
    // because the regex does not (and cannot, in a single Rust
    // `regex` pattern without look-around) inspect every row above
    // the info row for compose text. The compose-region rejection
    // lives in `compose_region_has_text`; the second test below
    // exercises that check directly.
    let cases: &[(&str, &str, bool)] = &[
        ("idle_editor", IDLE_EDITOR, true),
        ("idle_acp", IDLE_ACP, true),
        ("cold_start", COLD_START, true),
        ("agent_just_responded", AGENT_JUST_RESPONDED, true),
        ("api_aux_sidebar_wrap", API_AUX_SIDEBAR_WRAP, true),
        ("plan_mode", PLAN_MODE, true),
        ("composing", COMPOSING, true),
        ("composing_full", COMPOSING_FULL, true),
        ("composing_top", COMPOSING_TOP, true),
        ("composing_middle", COMPOSING_MIDDLE, true),
        ("home_after_typing", HOME_AFTER_TYPING, true),
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

#[test]
fn compose_region_check_rejects_compose_text() {
    // Direct test of the code-side compose-region second gate. The
    // regex alone accepts all composing fixtures (frame structure
    // is present); the readiness check rejects them via this gate.
    // This test pins the gate's coverage so the gap stays closed.
    let idle_cases: &[(&str, &str)] = &[
        ("idle_editor", IDLE_EDITOR),
        ("idle_acp", IDLE_ACP),
        ("cold_start", COLD_START),
        ("agent_just_responded", AGENT_JUST_RESPONDED),
        ("api_aux_sidebar_wrap", API_AUX_SIDEBAR_WRAP),
        ("plan_mode", PLAN_MODE),
    ];
    for (label, fixture) in idle_cases {
        assert!(
            !agentmux::tmux::compose_region_has_text(fixture),
            "{label}: expected no compose text, but compose_region_has_text returned true",
        );
    }

    let composing_cases: &[(&str, &str)] = &[
        ("composing", COMPOSING),
        ("composing_full", COMPOSING_FULL),
        ("composing_top", COMPOSING_TOP),
        ("composing_middle", COMPOSING_MIDDLE),
        ("home_after_typing", HOME_AFTER_TYPING),
    ];
    for (label, fixture) in composing_cases {
        assert!(
            agentmux::tmux::compose_region_has_text(fixture),
            "{label}: expected compose text, but compose_region_has_text returned false",
        );
    }
}
