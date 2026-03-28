# Coordination Tool Comparison Matrix

Last updated: 2026-03-28.

This matrix is a best-effort snapshot from public docs/readmes. Entries marked
`Not documented` indicate the capability was not explicitly stated in the
referenced materials (checked 2026-03-28).

| Project | Open Source | Primary Surface | Inter-Agent Messaging | MCP Server Surface | tmux-Oriented Runtime | ACP-Oriented Runtime |
|---|---|---|---|---|---|---|
| **agentmux** (this repo) | Yes (Apache-2.0) | CLI + MCP + TUI | **Yes** (relay envelope routing between sessions) | **Yes** | **Yes** | **Yes** |
| [waskosky/codex-cli-farm](https://github.com/waskosky/codex-cli-farm) | Yes (MIT) | Shell scripts + tmux workflows | No documented first-class relay protocol | No documented MCP server | **Yes** | No documented ACP runtime |
| [agentmux.app](https://agentmux.app/) | Source availability not published on site[^agentmuxapp] | Desktop/hosted UX | Not documented as first-class inter-agent relay messaging | Not documented | **Yes** (site states “Requires tmux”) | Not documented |
| [tmux-mcp-rs](https://docs.rs/crate/tmux-mcp-rs/0.1.1) | Yes | MCP server for tmux | No documented inter-agent relay protocol | **Yes** | **Yes** | No documented ACP runtime |
| [rinadelph/tmux-mcp](https://github.com/rinadelph/tmux-mcp) | Yes | MCP server for tmux | No documented inter-agent relay protocol | **Yes** | **Yes** | No documented ACP runtime |
| [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux) | Yes (AGPL-3.0-or-later; commercial license available)[^cmux] | Terminal/multiplexer app | No documented first-class relay messaging | Not documented | Not primarily tmux-based (supports terminal workflows; not tmux-dependent) | Not documented |
| [openai/symphony](https://github.com/openai/symphony) | Yes (Apache-2.0) | Work-orchestration framework for autonomous implementation runs | Not documented as first-class relay messaging | Not documented | Not documented | Not documented |
| [Contrabass](https://www.contrabass.dev/) | Yes (site indicates open source) | Web/CLI orchestration | Team/task coordination documented; direct session-to-session relay semantics unknown | Not documented | Supports tmux worker mode (`tmux` or `goroutine`) | Not documented |
| [Tide Commander](https://www.tide-commander.com/) | Mixed/free+paid offering[^tide] | Web desktop for coding agents | Multi-agent workflows documented; direct relay semantics unknown | Not documented | Not documented as tmux-oriented runtime | Not documented |

## Notes

- This project (`agentmux`) is differentiated by first-class inter-agent
  messaging contracts on top of host/runtime coordination.
- For tmux-only control MCP servers, compare `tmux-mcp-rs` and `tmux-mcp`
  directly against your sandbox and authorization needs.
- For orchestration UIs, verify whether they provide machine-consumable
  transport contracts versus UI-level coordination primitives.

## Sources

- https://github.com/waskosky/codex-cli-farm
  - tmux-centric Codex farm scripts, snapshot/restore, log fanout.
- https://agentmux.app/
  - product page; explicitly states “Requires tmux” and shows one-time license
    pricing.
- https://docs.rs/crate/tmux-mcp-rs/0.1.1
  - Rust tmux MCP server overview, install, and tooling scope.
- https://github.com/rinadelph/tmux-mcp
  - tmux MCP server repository and usage details.
- https://github.com/manaflow-ai/cmux
  - Ghostty-based macOS terminal with agent-oriented notifications; AGPL plus
    optional commercial license.
- https://github.com/openai/symphony
  - Open-source multi-agent work orchestration project (engineering preview)
    focused on isolated autonomous implementation runs.
- https://www.contrabass.dev/
  - orchestration product overview and positioning.
- https://www.tide-commander.com/
  - coding-agent desktop offering and pricing tiers.

[^agentmuxapp]: Site describes a license-backed product. Public source license
    information was not identified from the referenced pages.
[^tide]: Public marketing indicates a free tier and paid plans; source/license
    model is not clearly documented in the referenced pages.
[^cmux]: Repository README indicates AGPL-3.0-or-later and separately offers a
    commercial license for organizations unable to comply with AGPL.
