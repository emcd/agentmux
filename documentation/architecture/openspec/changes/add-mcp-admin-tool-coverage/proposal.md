## Why

The MCP admin meta-tools `updown`, `new`, and `change` are shipped: they are
composed into the router in `src/mcp/server/service.rs`, advertised by
`src/mcp/help.rs`, documented in `src/mcp/README.md`, and exercised by the
`mcp::updown` integration tests. But the canonical `mcp-tool-surface` spec's MCP
Tool Set requirement lists only `list`, `help`, `look`, `send`, `raww`, and
`choose`, so the advertised tool inventory is not contract-backed — a 0.9.0
stabilization risk and confusing for LLM clients (issues/mcp/10).

## What Changes

- Add `updown`, `new`, and `change` to the MCP Tool Set requirement.
- Add per-tool requirement blocks with request selectors, sender-authority and
  authorization notes, and success payload contracts, mirroring the shipped
  behavior documented in `src/mcp/README.md`.
- No behavior change: this is drift-to-shipped reconciliation only, so no code
  moves.

## Impact

- Affected spec: `mcp-tool-surface` (MODIFIED tool set; ADDED admin-tool
  requirement blocks).
- No code changes; the tools already exist and are tested.
- If a future change (for example `add-do-action-tool`) later modifies the same
  MCP Tool Set requirement, it writes its delta against the spec as it exists at
  that point — normal proposal hygiene, no special sequencing with this change.
