# Coordination Tool Comparison Matrix

Last updated: 2026-03-28.

This matrix is a best-effort snapshot from public docs/readmes. Entries marked
`Not documented` indicate the capability was not explicitly stated in the
referenced materials checked on 2026-03-28.

In this table, **ACP** refers to the **Agent Client Protocol** runtime surface
used by some coding-agent terminals. `✅` means documented support, `❌`
means no documented support, and `❔` means not documented in the
referenced materials. For tools clearly built around tmux or Ghostty, the ACP
column is marked `❌` unless public materials document ACP support.

| Project                                                               | License / Source Model         | Operator Surface                            | Execution Model                                                                     | Programmatic Interface                          | Inter-Agent Messaging | Terminal Stack | Supports ACP? |
| --------------------------------------------------------------------- | ------------------------------ | ------------------------------------------- | ----------------------------------------------------------------------------------- | ----------------------------------------------- | --------------------- | -------------- | ------------- |
| **agentmux** (this repo)                                              | Yes (Apache-2.0)               | CLI + TUI                                   | Host/runtime coordination for terminal sessions                                     | CLI + MCP send surface                          | ✅                     | tmux           | ✅             |
| [waskosky/codex-cli-farm](https://github.com/waskosky/codex-cli-farm) | Yes (MIT)                      | Shell scripts + tmux workflows              | Scripted tmux session farm for parallel coder terminals                             | Shell scripts; no documented machine-facing API | ❌                     | tmux           | ❌             |
| [agentmux.app](https://agentmux.app/)                                 | No[^agentmuxapp]               | Local terminal/desktop UX                   | Locally installed tmux-backed agent orchestration                                   | Not documented                                  | ❌                     | tmux           | ❌             |
| [tmux-mcp-rs](https://docs.rs/crate/tmux-mcp-rs/0.1.1)                | Yes (MIT)                      | MCP server for tmux                         | tmux sessions and panes exposed as MCP-controlled resources                         | **MCP server**                                  | ❌                     | tmux           | ❌             |
| [rinadelph/tmux-mcp](https://github.com/rinadelph/tmux-mcp)           | Yes (MIT)                      | MCP server for tmux                         | tmux sessions controlled through MCP tools and pane/session operations              | **MCP server**                                  | ❌                     | tmux           | ❌             |
| [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux)               | Yes (AGPL-3.0-or-later)[^cmux] | Terminal/multiplexer app                    | Native terminal/multiplexer environment for agent-driven workflows                  | Not documented                                  | ❌                     | Ghostty        | ❌             |
| [openai/symphony](https://github.com/openai/symphony)                 | Yes (Apache-2.0)               | Framework / CLI-oriented developer workflow | Autonomous work orchestration across isolated implementation runs                   | Not documented                                  | ❌                     | n/a            | ❔             |
| [Contrabass](https://www.contrabass.dev/)                             | Yes (Apache-2.0)               | CLI/TUI + embedded web dashboard            | Team runtime with tmux worker mode by default; goroutine worker mode also available | CLI + web dashboard; no documented MCP surface  | ❔                     | tmux           | ❌             |
| [Tide Commander](https://tidecommander.com/)                          | Yes (MIT)                      | Local visual orchestrator (web UI + CLI)    | Local multi-agent orchestration workspace                                           | Web UI + CLI; no documented MCP surface         | ❔                     | n/a            | ❔             |

## Notes

* This project (`agentmux`) is differentiated by first-class inter-agent
  messaging on top of host/runtime coordination.
* For tmux-oriented MCP control servers, compare `tmux-mcp-rs` and `tmux-mcp`
  directly against your sandbox, authorization, and pane-control needs.
* For orchestration UIs, verify whether they provide machine-consumable
  interfaces or primarily operator-facing coordination primitives.
* A `❔` marker indicates that the capability was not identified in the
  referenced public materials.

## Sources

* [https://github.com/waskosky/codex-cli-farm](https://github.com/waskosky/codex-cli-farm)

  * tmux-centric Codex farm scripts for running multiple coding agents, with
    snapshot/restore support, logging, and terminal-centric coordination.
* [https://agentmux.app/](https://agentmux.app/)

  * locally installed tmux-backed agent orchestration product for coordinating
    coding agents from the terminal or desktop.
* [https://docs.rs/crate/tmux-mcp-rs/0.1.1](https://docs.rs/crate/tmux-mcp-rs/0.1.1)

  * Rust MCP server that exposes tmux sessions and panes to LLM clients for
    inspection and control.
* [https://github.com/rinadelph/tmux-mcp](https://github.com/rinadelph/tmux-mcp)

  * tmux MCP server for controlling sessions, panes, and agent terminals from
    MCP-compatible clients.
* [https://github.com/manaflow-ai/cmux](https://github.com/manaflow-ai/cmux)

  * Ghostty-based macOS terminal with agent-oriented notifications; AGPL plus
    optional commercial license.
* [https://github.com/openai/symphony](https://github.com/openai/symphony)

  * Open-source multi-agent work orchestration project focused on isolated
    autonomous implementation runs.
* [https://www.contrabass.dev/](https://www.contrabass.dev/)

  * multi-agent coding orchestrator with team/task coordination, terminal-first
    workflows, and optional dashboard views.
* [https://tidecommander.com/](https://tidecommander.com/)

  * open-source, free-forever local multi-agent orchestration workspace with a
    visual interface and CLI support.

[^agentmuxapp]: Public materials reviewed for this comparison describe a
    commercial product, but public source availability was not identified.

[^cmux]: Repository README indicates AGPL-3.0-or-later and separately offers a
    commercial license for organizations unable to comply with AGPL.
