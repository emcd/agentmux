# Change: ACP client TUI for direct agent interaction

## Why
Debugging ACP issues currently requires the full relay + bundle + session stack. A standalone ACP client TUI cuts the relay out of the loop, enabling direct inspection of ACP agent I/O, faster smoke testing, and a reusable tool for testing any ACP-compatible agent. It also provides the human operator with a visual conversation interface identical in concept to the relay-mediated `look`/`raww` workflow.

## What Changes
- New binary `agentmux-acp`: a ratatui TUI that spawns an ACP agent, initializes the protocol, creates/loads a session, and provides an interactive prompt/response interface.
- Text input sends raw prompts directly to the ACP server (equivalent to future `raww` via relay).
- Scrollable output shows full conversation history with visual distinction between user and assistant messages.
- Extract `AcpStdioClient` into shared `src/acp/` module for reuse by relay delivery and the client binary.
- Permission request handling deferred to a follow-up (simple choice menu replacing text input).

## Impact
- Affected specs: new capability `acp-client`
- Affected code: `src/relay/delivery/acp_client.rs` (extract to `src/acp/`), new `src/bin/agentmux_acp.rs`, `Cargo.toml` (add `[[bin]]`)
