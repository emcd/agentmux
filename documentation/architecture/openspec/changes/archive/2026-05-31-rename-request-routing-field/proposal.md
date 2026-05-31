# Change: Rename request routing field from bundle_name to namespace

## Why

The D1d request-envelope routing field is named `bundle_name`, but after
identity federation (`add-identity-federation`) relay-wide principals
(`@GLOBAL`, `@EXTERNAL`, `@RELAY`) are valid participants that are not
bundle-scoped. The name `bundle_name` is misleading when the routing target
can be a relay-wide namespace. Renaming to `namespace` and expanding the
accepted value set unblocks delivering messages to `@GLOBAL` TUI/UI clients
from any bundle. Tracked as `todos/relay/58`.

Sequenced after `add-identity-federation` Slice 1 is fully merged.

## What Changes

- **BREAKING** (wire format): rename the request-frame envelope routing field
  `bundle_name` → `namespace` in `src/relay/stream.rs`
  (`IncomingEnvelope`/`IncomingFrame`), `src/relay/client.rs`, and
  `src/relay/connection.rs` (`resolve_effective_bundle` → renamed helper).
- Accepted values for `namespace` are expanded to include relay-wide namespace
  specifiers in addition to bundle names (see design.md for open questions on
  exact semantics).
- D1d routing resolution logic updated to reflect the new field name.
- MCP tools that pass a routing context (`look`, `raww`) updated to use
  `namespace` parameter name.
- MCP `send` tool gains optional `namespace` parameter to allow `@GLOBAL`
  targeting from bundle-bound sessions.
- CLI commands updated accordingly.

## Impact

- Affected specs: `session-relay`, `mcp-tool-surface`
- Affected code: `src/relay/stream.rs`, `src/relay/client.rs`,
  `src/relay/connection.rs`, MCP tool handlers
- Cross-lane: relay (primary), mcp, tui
- Dependency: `add-identity-federation` Slice 1 complete
