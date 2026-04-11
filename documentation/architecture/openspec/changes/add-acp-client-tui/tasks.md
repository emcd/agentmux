## 1. Shared ACP module extraction
- [ ] 1.1 Create `src/acp/mod.rs` and `src/acp/client.rs`
- [ ] 1.2 Move `AcpStdioClient`, `AcpRequestError`, `AcpPromptCompletion`, `AcpRequestResult`, and helper functions from `src/relay/delivery/acp_client.rs` to `src/acp/client.rs`
- [ ] 1.3 Update `src/acp/mod.rs` with re-exports
- [ ] 1.4 Update `src/relay/delivery/acp_client.rs` to re-export from `src/acp/`
- [ ] 1.5 Move `ACP_PROTOCOL_VERSION` constant to `src/acp/` module level
- [ ] 1.6 Verify existing relay ACP tests pass (cargo test)

## 2. TUI binary skeleton
- [ ] 2.1 Add `[[bin]] name = "agentmux-acp"` to `Cargo.toml`
- [ ] 2.2 Create `src/bin/agentmux_acp.rs` with CLI argument parsing (`--command`, `--session-id`, `--working-directory`)
- [ ] 2.3 Implement ACP agent spawn, initialize, session/new (or session/load)
- [ ] 2.4 Implement clean shutdown on Ctrl+C (close child process, restore terminal)

## 3. TUI rendering
- [ ] 3.1 Ratatui terminal setup with crossterm backend
- [ ] 3.2 Status bar: session ID, protocol status, worker state
- [ ] 3.3 Scrollable history area: user messages and assistant responses
- [ ] 3.4 Visual distinction: different background colors for user vs assistant messages
- [ ] 3.5 Text input area at bottom with Enter-to-send

## 4. ACP prompt interaction
- [ ] 4.1 Background task for `session/prompt` (non-blocking)
- [ ] 4.2 Channel-based update delivery from background task to TUI render loop
- [ ] 4.3 Stream response chunks into assistant message area
- [ ] 4.4 Session update notifications captured and rendered in history

## 5. Integration testing
- [ ] 5.1 Smoke test: `agentmux-acp --command "opencode acp"` with actual opencode ACP server
- [ ] 5.2 Verify `pty-debug` MCP tools can interact with running `agentmux-acp` TUI
