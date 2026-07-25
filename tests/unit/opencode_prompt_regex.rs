//! Compact table-driven coverage for the opencode prompt-readiness regex.
//!
//! The regex is shipped in `data/configuration/coders.toml` under the
//! `opencode` coder's `prompt-regex` field. The corrected shape anchors
//! on an empty current-input line immediately followed by the separator
//! and the status row that ends with `ctrl+p commands`. The corrected
//! readiness invariant: the current input prompt is empty and is the
//! `┃` line immediately before the separator.
//!
//! The fixture matrix keeps one composing entry and two agent-side
//! false-positive entries (agent processing, agent just responded) so
//! the corrected contract is preserved against future regressions.

use std::fs;

use regex::Regex;
use toml::Value;

const IDLE_SHORT_HISTORY: &str = "\
  ┃ Completely agree with you. The format of what landed was fine; the problem is that it should not have landed.
  ┃
  ┃
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)             ~/src/WORKTREES/agentmux/pty:pty
  /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

const IDLE_LONG_HISTORY: &str = "\
  2. Whether there's a trailing blank ┃ line that I'm missing (which would change the regex anchoring)
  3. Whether the inspect_lines = 10 window picks up the same shape I'm tracing

  If you can also share the output of agentmux look against the idle pc-setup-xo@infrastructure session (or whichever session you have open in Opencode), that's the cleanest signal — the same inspect_lines = 10 tail the regex sees.

  I'll hold on drafting until you've got it captured.
                                                                        ⊟ Build · MiniMax-M3 · 14.1s
  ┃
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)             ~/src/WORKTREES/agentmux/pty:pty
  /Users/me/src/WORKTREES/agentmux/pty                            133.6K (13%) · $0.27 ctrl+p commands    • OpenCode 1.18.4";

const COLD_START: &str = "\
  ┃
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)             ~/src/WORKTREES/agentmux/pty:pty
  /Users/me/src/WORKTREES/agentmux/pty                            133.6K (13%) · $0.27 ctrl+p commands    • OpenCode 1.18.4";

const COMPOSING: &str = "\
  ┃ Completely agree with you. The format of what landed was fine; the problem is that it should not have landed.
  ┃
  ┃ I suspect that our regex for prompt readiness is what needs tuning.
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)             ~/src/WORKTREES/agentmux/pty:pty
  /Users/me/src/WORKTREES/agentmux/pty                            65.6K (7%) · $0.27 ctrl+p commands    • OpenCode 1.18.3";

const AGENT_PROCESSING: &str = "\
  ┃ Completely agree with you. The format of what landed was fine; the problem is that it should not have landed.
  ┃
  ┃  ▣  Build · MiniMax-M3 · 14.1s
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)             ~/src/WORKTREES/agentmux/pty:pty
  /Users/me/src/WORKTREES/agentmux/pty                            133.6K (13%) · $0.27 ctrl+p commands    • OpenCode 1.18.4";

const AGENT_JUST_RESPONDED: &str = "\
  ┃ user prompt
  ┃  agent response text
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
  Build · MiniMax-M3 MiniMax Token Plan (minimax.io)             ~/src/WORKTREES/agentmux/pty:pty
  /Users/me/src/WORKTREES/agentmux/pty                            133.6K (13%) · $0.27 ctrl+p commands    • OpenCode 1.18.4";

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
        ("idle_short_history", IDLE_SHORT_HISTORY, true),
        ("idle_long_history", IDLE_LONG_HISTORY, true),
        ("cold_start", COLD_START, true),
        ("composing", COMPOSING, false),
        ("agent_processing", AGENT_PROCESSING, false),
        ("agent_just_responded", AGENT_JUST_RESPONDED, false),
    ];

    for (label, fixture, expected_match) in cases {
        let actual = regex.is_match(fixture);
        assert_eq!(
            actual, *expected_match,
            "fixture {label}: expected match={expected_match}, got match={actual}",
        );
    }
}
