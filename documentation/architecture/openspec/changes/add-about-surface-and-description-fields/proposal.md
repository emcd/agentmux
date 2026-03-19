# Change: Add about surfaces and configuration description fields

## Why

Operators and agents need a reliable runtime-native way to inspect bundle and
session purpose metadata. Today that context often lives in external notes that
can drift from active configuration.

## What Changes

- Add optional configuration fields:
  - bundle-level `description`
  - session-level `description` on `[[sessions]]`
- Add CLI read surface:
  - `agentmux about`
  - `agentmux about --session <session-id>`
  - optional `--bundle <bundle-id>` selector with same-bundle MVP lock
- Add MCP read surface:
  - tool `about` with optional `session_id` and `bundle_name`
- Add relay `about` operation contract with deterministic response schema,
  selector semantics, and error taxonomy.
- Reuse authorization capability `list.read` for `about` in MVP.

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Affected code (implementation follow-up):
  - relay request/response handling for `about`
  - configuration parsing/validation for description fields
  - CLI/MCP adapters for `about` request/response mapping
