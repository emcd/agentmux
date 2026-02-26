# Change: Add MCP-first session relay MVP

## Why

LLM coding clients need a local, tool-driven way to exchange messages even when
their harnesses do not expose hook systems. A tmux-backed relay lets MCP tools
inject visible prompts into agent sessions while staying simple and local-first.

## What Changes

- Add a new `session-relay` capability for `tmuxmux`.
- Define session-level routing as the external addressing primitive.
- Support directed delivery to one or more selected sessions, in addition to
  full-bundle broadcast.
- Define strict, pretty-printed JSON chat envelopes for injected messages.
- Define quiescence-gated delivery so messages are not injected during active
  output bursts.
- Document quiescence caveats for dynamic pane output such as clock-style
  statusline content.
- Define MCP-first operations for bundle management and message delivery.
- Define configurable tmux socket selection for server operations.
- Exclude transport/accept/done ACK protocols from MVP.
- Exclude urgent quiescence-bypass overrides from MVP (future change).
- Exclude crash-recovery durability for queued messages from MVP.

## Impact

- Affected specs: `session-relay` (new capability).
- Affected code:
  - MCP server surface for bundle/session/message operations.
  - tmux adapter for session lifecycle, pane resolution, capture, and
    `send-keys` injection.
  - delivery scheduler logic for quiescence checks and timeout handling.
