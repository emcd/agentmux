## Why

When a bundle relay is unreachable, the CLI and the MCP server each synthesize a
`down` bundle entry for the home bundle and stamp it with a `state_reason_code`
that distinguishes "the relay was never started" from "the relay is present but
not answering". Both surfaces implement the same two-arm rule, and no live
requirement names either value.

The corpus therefore requires the field without constraining it: `cli-surface`
and `mcp-tool-surface` both list `state_reason_code` as required when
`state=down`, and `bundle-lifecycle` Bundle Down Reason Precedence scopes itself
explicitly to the codes *relay* reports. A caller reading the specs cannot learn
that `not_started` and `relay_unavailable` exist, and nothing stops the two
surfaces from drifting apart on a value clients branch on.

The rule was specified once, in a change archived 2026-04-17, and never reached
any live spec.

## What Changes

- State the client-synthesized down-reason mapping as a live requirement, owned
  by the capability that already owns canonical list-payload semantics.
- Scope it explicitly against the relay-reported `runtime_*` codes, so the two
  reason vocabularies are not read as one namespace with a missing precedence
  rule.

No behavior change. Both code paths already implement the mapping; this closes
the gap between what they emit and what the corpus constrains.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `addressing-routing`: adds a requirement fixing the `state_reason_code` values
  a client surface stamps on a home bundle it synthesizes as `down` because the
  relay is unreachable, and their derivation from relay-socket presence.

## Impact

Documentation-only for this change. The behavior is already implemented at
`src/commands/list.rs` (`synthesize_unreachable_bundle`) and
`src/mcp/server/handlers/list.rs` (`synthesize_down_bundle`), and described in
`src/mcp/README.md`. Adding the requirement makes those two paths accountable to
one statement rather than to each other.
