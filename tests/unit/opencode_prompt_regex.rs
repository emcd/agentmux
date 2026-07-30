//! Compact table-driven coverage for the opencode prompt-readiness regex.
//!
//! The regex is shipped in `data/configuration/coders.toml` under the
//! `opencode` coder's `prompt-regex` field. It matches the Opencode
//! prompt UI frame structure: at least one `┃` line, followed by the
//! separator `╹▀▀▀...`, followed by the status row that ends with
//! `ctrl+p commands    • OpenCode <version>`.
//!
//! The regex does NOT distinguish between empty-input (idle) and
//! non-empty-input (composing) states -- that distinction is the
//! cursor-column check wired in `src/tmux/quiescence_probe.rs` via
//! `prompt-idle-column = 5`. The full readiness check (regex match +
//! cursor column equals 5) is the gate that prevents delivery during
//! composing; see `tests/integration/relay_delivery_prompt.rs` for the
//! cursor-column mismatch timeout test.
//!
//! Frame-structure matching (rather than "blank input box above info row")
//! is required because the input-box area in some layouts carries
//! sidebar content (e.g. when the working-directory path wraps into
//! the bottom of the input area). The strict "blank above info row"
//! anchor cannot distinguish sidebar text from compose-box text, since
//! both render inside `┃` rows.

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
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
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
