# Coordination Tool Comparison Matrix

Last updated: 2026-03-28.

This matrix is a best-effort snapshot from public docs/readmes. Entries marked
`Not documented` indicate the capability was not explicitly stated in the
referenced materials checked on 2026-03-28.

| Project                                                               | License / Source Model                                       | Operator Surface                            | Execution Model                                                                     | Transport / API Surface                         | Inter-Agent Messaging                                                                   | tmux-Oriented Runtime                                               | ACP-Oriented Runtime      |
| --------------------------------------------------------------------- | ------------------------------------------------------------ | ------------------------------------------- | ----------------------------------------------------------------------------------- | ----------------------------------------------- | --------------------------------------------------------------------------------------- | ------------------------------------------------------------------- | ------------------------- |
| **agentmux** (this repo)                                              | Yes (Apache-2.0)                                             | CLI + MCP + TUI                             | Relay host delivering messages to tmux-backed or ACP-backed terminal sessions       | CLI + MCP send surface                          | **Yes** (relay envelope routing between sessions)                                       | **Yes**                                                             | **Yes**                   |
| [waskosky/codex-cli-farm](https://github.com/waskosky/codex-cli-farm) | Yes (MIT)                                                    | Shell scripts + tmux workflows              | Scripted tmux session farm for parallel coder terminals                             | Shell scripts; no documented machine-facing API | No documented first-class relay protocol                                                | **Yes**                                                             | No documented ACP runtime |
| [agentmux.app](https://agentmux.app/)                                 | Source availability not published on site[^agentmuxapp]      | Desktop/hosted UX                           | Host-managed agent orchestration over tmux-backed terminals                         | Not documented                                  | Not documented as first-class inter-agent relay messaging                               | **Yes** (site states “Requires tmux”)                               | Not documented            |
| [tmux-mcp-rs](https://docs.rs/crate/tmux-mcp-rs/0.1.1)                | Yes (MIT)                                                    | MCP server for tmux                         | tmux sessions and panes exposed as MCP-controlled resources                         | **MCP server**                                  | No documented first-class relay/message-bus semantics; control is pane/session oriented | **Yes**                                                             | No documented ACP runtime |
| [rinadelph/tmux-mcp](https://github.com/rinadelph/tmux-mcp)           | Yes (MIT)                                                    | MCP server for tmux                         | tmux sessions controlled through MCP tools and pane/session operations              | **MCP server**                                  | No documented first-class relay/message-bus semantics; control is pane/session oriented | **Yes**                                                             | No documented ACP runtime |
| [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux)               | Yes (AGPL-3.0-or-later; commercial license available)[^cmux] | Terminal/multiplexer app                    | Native terminal/multiplexer environment for agent-driven workflows                  | Not documented                                  | No documented first-class relay messaging                                               | Not primarily tmux-based (terminal workflows; not tmux-dependent)   | Not documented            |
| [openai/symphony](https://github.com/openai/symphony)                 | Yes (Apache-2.0)                                             | Framework / CLI-oriented developer workflow | Autonomous work orchestration across isolated implementation runs                   | Not documented                                  | Not documented as first-class relay messaging                                           | Not documented                                                      | Not documented            |
| [Contrabass](https://www.contrabass.dev/)                             | Yes (Apache-2.0)                                             | CLI/TUI + embedded web dashboard            | Team runtime with tmux worker mode by default; goroutine worker mode also available | CLI + web dashboard; no documented MCP surface  | Team/task coordination documented; direct session-to-session relay semantics unknown    | **Yes** (default tmux worker mode; goroutine alternative available) | Not documented            |
| [Tide Commander](https://tidecommander.com/)                          | Yes (MIT)                                                    | Local visual orchestrator (web UI + CLI)    | Local multi-agent orchestration workspace                                           | Web UI + CLI; no documented MCP surface         | Multi-agent workflows documented; direct relay semantics unknown                        | Not documented as tmux-oriented runtime                             | Not documented            |

## Notes

* This project (`agentmux`) is differentiated by first-class inter-agent
  messaging contracts on top of host/runtime coordination.
* For tmux-oriented MCP control servers, compare `tmux-mcp-rs` and `tmux-mcp`
  directly against your sandbox, authorization, and pane-control needs.
* For orchestration UIs, verify whether they provide machine-consumable
  transport contracts versus UI-level coordination primitives.
* Some projects document multi-agent coordination at the workflow or operator
  level without documenting a first-class agent-to-agent relay or mailbox
  protocol. This matrix treats those as distinct capabilities.

## Sources

* [https://github.com/waskosky/codex-cli-farm](https://github.com/waskosky/codex-cli-farm)

  * tmux-centric Codex farm scripts for running multiple coding agents, with
    snapshot/restore support, logging, and terminal-centric coordination.
* [https://agentmux.app/](https://agentmux.app/)

  * tmux-backed agent orchestration product for coordinating coding agents from
    a desktop/hosted experience.
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

[^agentmuxapp]: Site describes a license-backed product. Public source-license
    information was not identified from the referenced pages.

[^cmux]: Repository README indicates AGPL-3.0-or-later and separately offers a
    commercial license for organizations unable to comply with AGPL.
