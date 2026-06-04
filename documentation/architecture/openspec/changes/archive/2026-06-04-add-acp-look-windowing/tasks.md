## 1. Relay contract and windowing

- [x] 1.1 Add `offset: Option<usize>` to `RelayRequest::Look`; thread through
  `LookRequestContext` and `normalize_request_identities`
- [x] 1.2 Add required `entries_total` and `returned_entries_count` to
  `LookSnapshotPayload::AcpEntriesV1`
- [x] 1.3 Window ACP entries via two saturating subtractions in
  `derive_acp_look_snapshot`; carry `entries_total`/`returned_entries_count` into
  every return branch (unavailable, recovering, no-buffer, fresh)
- [x] 1.4 Apply transport-keyed defaults (tmux 120, ACP 50); reject nonzero
  offset on tmux targets with `validation_offset_unsupported`

## 2. MCP/CLI surfaces

- [x] 2.1 Add `offset` to MCP `LookParams`; forward to the relay request
- [x] 2.2 Surface `entries_total`/`returned_entries_count` on MCP and CLI look
  responses
- [x] 2.3 Carry mechanical `offset: None` on TUI/CLI look construction sites
- [x] 2.4 Document params and metadata in `src/mcp/README.md`

## 3. Tests

- [x] 3.1 Relay ACP offset window + metadata backward walk
  (`tests/integration/acp/look.rs`)
- [x] 3.2 tmux nonzero-offset rejection and zero-offset acceptance
  (`tests/unit/relay.rs`)
- [x] 3.3 MCP offset forwarding + metadata passthrough
  (`tests/integration/mcp/look.rs`)
- [x] 3.4 CLI metadata passthrough (`tests/integration/cli/look.rs`)
