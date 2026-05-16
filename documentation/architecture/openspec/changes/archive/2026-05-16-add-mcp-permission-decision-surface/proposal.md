# Change: Add MCP Permission Decision Surface (Operator-Class)

## Why

ACP permission queueing and resolution is fully wired at the relay
(`add-acp-permission-request-ui-mediated`, archived), but decision authority is
restricted to `client_class=ui` principals. The Coordinator and Agents Engineer
sessions cannot programmatically observe pending requests or submit decisions,
which blocks:

- automated integration testing of the permission flow without a TUI in the
  loop,
- coordinator oversight of long-running ACP turns blocked on permission,
- batch or scripted approval workflows in agent-driven environments.

Simply relaxing the UI-only gate to admit generic `client_class=agent` would
collapse the decision boundary onto the same class that hosts coder agents,
opening a confused-deputy path under any `grant` policy mis-configuration. The
architecture assessment at `coordination/mcp/7` recommends introducing a third
client class explicitly authorized for decisioning, and adding a polling-based
MCP tool surface.

Coordinator decisions on the four open questions (delivered 2026-05-14):

1. Authorization model: new `client_class=operator` (not per-principal
   capability bit).
2. Visibility model: pull-only polling RelayRequest variants (push deferred).
3. Sequencing: separate follow-on proposal (not Rev2 of the archived UI proposal).
4. Operator session defaults: `master` and `mcp` lanes only.

Primary reference standard:
- ACP Tool Calls: https://agentclientprotocol.com/protocol/tool-calls.md

## What Changes

### Relay (`session-relay` capability)

- Add `RelayClientClass::Operator` alongside existing `Agent` / `Ui`.
- Add operator-class hello validation: a stream MAY claim `operator` only if
  the bundle policy preset for that principal explicitly authorizes it; invalid
  claims fail with `validation_invalid_client_class_for_hello`.
- Modify the decision submitter gate to accept `client_class ∈ {ui, operator}`
  while keeping `grant` policy as the binding authorization check.
- Add `RelayRequest::PermissionList` for polling visibility of pending
  permission requests, scoped same-bundle, gated on grant-authorized
  `{ui, operator}` principals.
- Permission stream events (`permission.snapshot`, `permission.requested`,
  `permission.resolved`) remain UI-only on the push channel for alpha; operator
  visibility is poll-only.
- Preserve all existing sender-authority, association-derived `decided_by`,
  cross-bundle rejection, and error-taxonomy contracts unchanged.
- Update arbitration and denial scenario wording to reflect that the first
  authorized `{ui, operator}` decision wins.

### MCP (`mcp-tool-surface` capability)

- Add top-level MCP tool `grant` with subcommands `list` and `resolve`,
  following the existing `list`-with-`command` meta-tool pattern. The tool
  name mirrors the `grant` policy capability that gates it, matching the
  one-tool-per-policy-control naming used by `list`, `look`, `send`, `raww`.
- `grant` (with `command="list"`) returns pending permission requests for
  the associated bundle.
- `grant` (with `command="resolve"`) submits an ACP-native decision
  (`outcome=selected` with `option_id`, or `outcome=cancelled`).
- Sender authority MUST remain association-derived; payload-supplied identity
  fields fail with `validation_invalid_params`.
- Relay error taxonomy passes through unchanged
  (`validation_invalid_params`, `validation_cross_bundle_unsupported`,
  `validation_invalid_client_class_for_action`, `authorization_forbidden`,
  `runtime_permission_request_already_resolved`,
  `runtime_permission_queue_full`, `runtime_permission_queue_unavailable`).
- `help` query catalog adds `grant`, `grant.list`,
  `grant.resolve`.

### Out of Scope

- Push-based stream-event subscription for `client_class=operator`.
- Auto-expiry timers, multi-party voting, cross-bundle decisioning, delegation.
- Adapter-side authorization rules; relay remains the sole authorization point.

## Impact

- Affected specs:
  - `session-relay` (decision gate, hello validation, polling RelayRequest)
  - `mcp-tool-surface` (new tool contract and help catalog entry)
- Affected code (implementation follow-up):
  - relay client-class taxonomy extension and hello validation
  - new `PermissionList` request handler reusing
    `list_pending_permission_requests`
  - relay decision gate relaxation (`{ui, operator}`)
  - relay policy preset wiring to allow operator-class claim per principal
  - MCP `grant` tool plumbing including help catalog
  - integration coverage for operator-class hello, list, resolve, error
    passthrough, sender-authority discipline, and cross-bundle rejection
