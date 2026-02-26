# Change: Add RFC 822/MIME pane envelope format

## Why

Injected pane messages need to be readable for humans attached to tmux and
strictly parseable for agents. A header-based envelope with MIME parts enables
human-friendly addressing (`To`/`Cc` display names) while keeping canonical
machine metadata in structured JSON.

## What Changes

- Add a new `pane-envelope` capability for injected message formatting.
- Define compact JSON manifest preamble as envelope start marker.
- Define an RFC 822-style header block for human-visible addressing metadata.
- Define MIME multipart framing for extensible payload composition.
- Define MIME closing boundary as envelope end marker.
- Define canonical compact JSON manifest fields for machine parsing.
- Define `Cc` header semantics as informational (display) metadata.
- Reserve additional MIME part types for future pointer-based attachments.
- Define prompt batching and splitting under configurable token budget.
- Define strict validation rules for pane parsing.

## Impact

- Affected specs: `pane-envelope` (new capability).
- Related specs:
  - `session-relay` from `add-mcp-session-relay-mvp`
  - `mcp-tool-surface` from `add-mcp-tool-surface-contract`
- Affected code:
  - envelope renderer for pane injection
  - envelope parser for agent-side/relay-side interpretation
  - batching layer for token-budget prompt packing
  - validation layer for headers, boundaries, and manifest content
