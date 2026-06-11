# Change: Retire bundle_name from stream events and response types

## Why

`RelayStreamEvent.bundle_name` is the event-side residue of the bare-id legacy
that `add-relay-routing-layer` retired on the request side. The "Canonical
Session Identity" requirement already mandates that `target_session` in all
wire output carries the `session@bundle` form, making `bundle_name` on events
fully redundant. The same redundancy applies to `bundle_name` fields on Send,
Look, and PermissionList responses, where the canonical session ids (requester
and target) already encode the bundle context.

Code investigation confirms the TUI never reads `event.bundle_name` for any
filtering or routing decision — it is dead data on the consumer side. Removing
it shrinks the wire shape and eliminates a source of confusion about what the
field means (its semantics were muddied by the Step 8 GLOBAL namespace change).

## What Changes

- **BREAKING** Remove `bundle_name` from `RelayStreamEvent` top-level fields.
- **BREAKING** Remove `bundle_name` from `RelayResponse::Send`.
- **BREAKING** Remove `bundle_name` from `RelayResponse::Look`.
- **BREAKING** Remove `bundle_name` from `RelayResponse::PermissionList`.
- **BREAKING (MCP clients)** Remove `bundle_name` from MCP `send`, `look`, and
  `grant list` tool output; MCP callers that parse this field will see it absent.
- Qualify `target_session` in stream event payloads that emit it in bare form
  (permission events, delivery events for bundle-bound targets).
- Update TUI and MCP test fixtures that construct `RelayStreamEvent` with bare
  `target_session` values to use canonical `session@bundle` form.

No compatibility window; alpha software.

## Impact

- Affected specs: `session-relay`, `mcp-tool-surface`
- Affected code: `src/relay/contract.rs`, `src/relay/delivery/`,
  `src/relay/handlers/`, `src/relay/stream.rs`, `src/tui/state/` (test
  fixtures only), `src/mcp/server/handlers/{send,look,grant}.rs`,
  `src/commands/send.rs`, `src/commands/look.rs` (CLI JSON output and send's
  `bundle=` text line removed), `tests/integration/mcp/{send,look,grant}.rs`
- Cross-lane: relay (BE), TUI (FE), MCP (AE)
