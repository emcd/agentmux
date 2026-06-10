## Context

`RelayStreamEvent` carries a top-level `bundle_name` field alongside
`target_session`. The "Canonical Session Identity" requirement already mandates
that `target_session` emits `session@bundle` form everywhere. `bundle_name` is
therefore fully derivable from the suffix of `target_session` and encodes no
independent information.

The same redundancy holds for response-level `bundle_name` on Send, Look, and
PermissionList: the requester and target session ids are canonical, and the
bundle is recoverable from their suffixes.

## Goals / Non-Goals

- Goals: eliminate `bundle_name` from `RelayStreamEvent` and the three
  response variants; ensure all `target_session` values in events are
  canonical before the field is removed.
- Non-Goals: removing `bundle_name` from relay request parameters
  (`PermissionResolve`, `PermissionList`, `IdentityIntrospect`); that is a
  separate exercise and may affect routing semantics.

## Decisions

- **No compatibility window.** Alpha software. Relay and client lanes (TUI, MCP)
  change together; no dual-emit period.
- **Sequencing within BE commit:** qualify event `target_session` values first
  (permission events, delivery events for bundle-bound targets), then remove
  `bundle_name` from the struct. A single commit is fine since no deployed
  clients pin to the old shape.
- **TUI behavioral impact is zero.** Code audit confirms `event.bundle_name` is
  never read in `record_stream_events` or any event consumer. Only test
  fixtures (inline `RelayStreamEvent { bundle_name: "agentmux", ... }`) need
  updating; those tests also need their `target_session` values qualified since
  the assertion logic uses `event.target_session` directly.
- **MCP passthrough audit.** MCP send, look, and grant-list handlers explicitly
  destructure `bundle_name` from relay responses and re-emit it in tool output;
  it will not fall away with the struct change. Removal requires targeted edits
  to three handler files and their integration test fixtures.

## Risks / Trade-offs

- Any external client that parses `bundle_name` from stream events or Send/Look
  responses will see a missing field. Alpha: acceptable. Document in the commit
  message.
- The TOP-LEVEL `target_session` on all three permission event types
  (permission.snapshot, permission.requested, permission.resolved) is bare in
  current relay code — it carries the routing addressee (a UI session id)
  without `@bundle`. Qualifying it is required before removal gives clients no
  other way to recover the namespace. The PAYLOAD `target_session` inside
  permission.requested (the ACP session awaiting permission) is already
  canonical via `canonical_session_id()`; task 1.1 must qualify only the
  top-level field.
- The sender-facing delivery_outcome (async_worker.rs:453) routes to the
  sender's bundle but describes a target in a potentially different bundle.
  Its `target_session` must be qualified as `session@TARGET_BUNDLE`, not
  `session@SENDER_BUNDLE`, before `bundle_name` removal eliminates the only
  existing cross-bundle namespace disambiguator on this event path.

## Open Questions

- None blocking. See proposal for cross-lane sequencing.
