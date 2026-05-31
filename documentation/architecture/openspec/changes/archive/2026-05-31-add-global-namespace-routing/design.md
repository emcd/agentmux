## Context

`rename-request-routing-field` renamed the wire routing selector `bundle_name`
→ `namespace` and introduced the vocabulary of relay-wide namespace specifiers
(`GLOBAL`, `EXTERNAL`, `RELAY`). GLOBAL delivery was stubbed with
`validation_namespace_routing_unavailable` pending a non-bundle dispatch path.
During MCP implementation, `namespace` was also added as an explicit parameter
on `send`, `look`, and `raww` tool schemas — a design error identified during
review.

The relay's `StreamRegistry` already stores relay-wide sessions under
`RegistryKey::RelayWide { principal_id }` and the `registry_key_for_target`
helper already classifies `@GLOBAL` principal IDs as relay-wide keys. The
plumbing exists; it only needs to be wired into the send dispatch path.

## Goals / Non-Goals

Goals:
- Route `@GLOBAL` targets in `Send` to the relay-wide registry via suffix
  inference (no explicit `namespace = "GLOBAL"` required from the client).
- Remove `namespace` from `send`, `look`, and `raww` MCP tool schemas.
- Implement `List` for `namespace = "GLOBAL"` (relay-wide session list).
- Retire `validation_namespace_routing_unavailable`.

Non-Goals:
- `Look`/`Raww` to `@GLOBAL` targets (relay-wide UI sessions are not
  tmux-pane-backed).
- `broadcast = true` under GLOBAL (D6 from rename design.md).
- Mixed-namespace fan-out in one request (deferred to `designs/relay/6`).
- Fine-grained relay-wide send authorization (deferred; see Risks).

## Decisions

**D1 — Suffix-based routing, not a parallel dispatch branch.**
The relay does not need a separate `dispatch_global_request` branch. Instead,
the existing send path in `handle_send` resolves each target via the
`registry_key_for_target` helper, which already returns `RegistryKey::RelayWide`
for `@GLOBAL` principal IDs. The change is to make `handle_send` look up and
deliver to `RelayWide` keys in addition to `Session` keys — the dispatch stays
in the same code path.

**D2 — Per-target suffix inference; no required `namespace` on Send/Look/Raww.**
A caller specifying `targets = ["operator@GLOBAL"]` carries the routing context
in the target ID. The relay infers GLOBAL routing from the `@GLOBAL` suffix.
Bare targets (no `@<namespace>`) default to the sender's bound bundle; relay-wide
senders without a bound bundle and bare targets receive
`validation_missing_routing_namespace`. The wire `namespace` field on
Send/Look/Raww is ignored by the relay for target resolution.

**D3 — Mixed-namespace targets in one Send: error this slice.**
If a single Send contains both `@GLOBAL` and `@<bundle>` targets, relay returns
`validation_conflicting_namespaces`. Full cross-namespace fan-out is deferred
to `designs/relay/6`. This preserves D5 from the rename proposal while
implementing the suffix-inference mechanism.

**D4 — List with `namespace = "GLOBAL"`: return relay-wide sessions.**
`List` has no target principal IDs to infer from, so the explicit `namespace`
selector is the right mechanism. `namespace = "GLOBAL"` returns all currently
registered relay-wide sessions. Resolves `todos/relay/61`.

**D5 — Authorization: permit-all for authenticated sessions (this slice).**
Any session that has completed Hello registration may send to `@GLOBAL` targets.
Fine-grained relay-wide send authorization is deferred.

**D6 — Error code retirement.**
`validation_namespace_routing_unavailable` is removed; no migration needed
(alpha software; it was explicitly temporary).

## Risks / Trade-offs

- **Removing `namespace` from MCP tools is breaking**: any MCP caller passing
  `namespace` on `send`/`look`/`raww` will stop having it honoured. Acceptable
  given alpha status and that the field was a design error.
- **Mixed-namespace error is new**: callers that attempt cross-namespace fan-out
  receive a new typed error. Correct behaviour; documents the limitation.
- **Relay-wide send authz is permit-all**: all authenticated agents can reach
  the operator. Acceptable for alpha / single-operator deployments.
