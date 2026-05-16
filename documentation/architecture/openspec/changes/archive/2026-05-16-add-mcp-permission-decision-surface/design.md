## Context

ACP permission requests are transport-level pauses on in-flight ACP turns. The
relay queues them durably (`permission_queue.json`), exposes them to
grant-authorized UI streams via `permission.snapshot` and
`permission.requested` events, and accepts terminal decisions via
`RelayRequest::PermissionResolve`. The archived
`add-acp-permission-request-ui-mediated` locks UI-class submitter identity,
non-spoofable `decided_by`, same-bundle scope, bounded queue, non-expiring
pending lifecycle, and ACP-native decision outcomes.

This change extends decision authority to a new `operator` client class so
non-TUI operator principals (coordinator, agents-engineer) can observe and
resolve permissions via the MCP surface, without collapsing decision authority
onto generic coder agents.

Normative external reference:
- ACP Tool Calls: https://agentclientprotocol.com/protocol/tool-calls.md

Implementation guidance:
- Implementers MUST treat ACP Tool Calls as source-of-truth for
  `session/request_permission` outcomes (`selected` + `optionId`,
  `cancelled`) and permission option kinds. This change adds a
  non-UI submitter path; it does NOT change the ACP outcome contract.

## Goals

- Provide a programmatic decision-authority surface for the Coordinator and
  Agents Engineer lanes without weakening the existing UI-only guarantee for
  generic coder agents.
- Keep relay as the sole authorization point.
- Keep `grant` policy capability as the binding authorization check; class
  remains defense-in-depth.
- Provide a polling-based visibility primitive that fits the MCP request/reply
  idiom.
- Preserve sender-authority discipline, cross-bundle rejection, error
  taxonomy, and ACP option fidelity unchanged.

## Non-Goals

- Push event subscription for `client_class=operator`.
- Cross-bundle permission decisioning.
- Multi-party voting/consensus.
- Per-principal capability bit on `Agent` class.
- Adapter-side authorization rules.
- New sender-facing sync response shapes beyond current send contract.

## Decisions

### 1. New `client_class=operator`

- `RelayClientClass` becomes `{Agent, Ui, Operator}`.
- A stream MAY register as `Operator` only if the bundle policy preset for
  that principal explicitly authorizes operator-class registration.
- Invalid operator claims fail at hello with
  `validation_invalid_client_class_for_hello` and the stream is rejected
  before any request/event handling.
- The policy preset mechanism (existing operator preset) is the authorization
  source; relay does not trust client-supplied class.

Rationale: class is a property of *who you are* at the relay layer; capability
is a property of *what you may do*. Decision authority is a role question.
A capability bit on `Agent` would leave a confused-deputy path open via policy
mis-assignment to a coder agent — the very risk this proposal aims to remove.

### 2. Decision Gate Relaxes to `{ui, operator}`

- `permission.resolve` accepts `client_class ∈ {ui, operator}`; rejects
  `agent` with the existing `validation_invalid_client_class_for_action` code.
- `authorize_grant` policy check remains unchanged and binding. An operator
  principal without `grant=all:home` is denied with `authorization_forbidden`
  exactly as a UI principal would be.

### 3. Polling Visibility via `RelayRequest::PermissionList`

- New `RelayRequest::PermissionList`:
  - Requires associated principal with `client_class ∈ {ui, operator}`.
  - Same-bundle scope only.
  - Requires `grant` capability evaluated against the listing principal.
  - Returns the persisted pending records (the same data published in
    `permission.snapshot` and `permission.requested` payloads).
- Push events remain UI-only on the stream channel for alpha. Operator clients
  poll. The polling response payload SHALL match the field set of
  `permission.requested` payloads to keep contract surfaces aligned.

### 4. MCP Tool Surface: Single `grant` Tool with Subcommands

- New top-level MCP tool `grant`.
- Subcommand selector: `command="list"` or `command="resolve"`.
- `grant` with `command="list"` returns pending requests.
- `grant` with `command="resolve"` submits decisions and returns the
  relay decision response.
- Follows the existing `list` meta-tool pattern (`list` with
  `command="sessions"`).

Rationale: the tool name `grant` mirrors the `grant` policy capability that
authorizes it — every existing tool name (`list`, `look`, `send`, `raww`)
matches the policy control it exercises, so the permission-decisioning tool
takes the name of the `grant` control. A single tool groups operator
decisioning under one namespace, mirrors the established `list` pattern, and
leaves the door open for future subcommands (`grant.snapshot`,
`grant.cancel_all`, etc.) without churning the tool inventory.

### 5. Sender Authority Discipline

- MCP `grant resolve` MUST NOT accept payload identity fields
  (`ui_session_id`, `operator_session_id`, `decided_by`); these fail with
  `validation_invalid_params`, consistent with existing `raww`/`send`
  contract.
- Relay derives `decided_by` from the association-bound principal session id.
- The MCP `grant resolve` response echoes `decided_by` from relay
  unchanged; it does not mint or transform actor identity.

### 6. Error Taxonomy Passthrough

MCP `grant` preserves relay codes unchanged:

- `validation_invalid_params`
- `validation_cross_bundle_unsupported`
- `validation_invalid_client_class_for_action`
- `validation_invalid_client_class_for_hello` (relay registration phase only)
- `authorization_forbidden` with `capability="grant"`
- `runtime_permission_request_already_resolved`
- `runtime_permission_queue_full`
- `runtime_permission_queue_unavailable`

### 7. Same-Bundle Scope

- `grant list` and `grant resolve` reject cross-bundle targets with
  `validation_cross_bundle_unsupported`, consistent with `raww`/`send`.

### 8. Operator Session Defaults

- Bundle policy preset SHALL grant operator-class hello to the Coordinator
  (`master`) and Agents Engineer (`mcp`) lanes only.
- Specialist lanes (`relay`, `tui`, `pty`/`acp`) and the TUI global `user`
  session do not claim operator class by default.
- Operator-class claim and `grant=all:home` are independent gates; both must
  be configured for decision authority.

### 9. Replay/Dedupe Semantics for Operator

- Push events stay UI-only in alpha; no operator replay contract is added.
- The `grant list` response is a point-in-time snapshot; callers
  observe live state on each call.
- Idempotency on already-resolved requests is enforced by relay via
  `runtime_permission_request_already_resolved`; MCP passes through.

### 10. Inscription Audit Tags

- Existing `relay.permission.requested` and `relay.permission.resolved`
  inscription tags carry `decided_by` (session id) which already distinguishes
  UI vs. operator-class resolutions without a new tag.
- No new audit tag added in this change. Reconsider if future analytics
  require operator-vs-UI segregation at the inscription level.

## Risks / Trade-offs

- **Class taxonomy widening risk.** Adding a third client class touches
  hello-frame validation, registry resolution, and routing.
  - Mitigation: keep operator-class routing inactive for delivery (no `send`
    routes to operator). Operator class is decision-submitter-only in alpha.
  - Mitigation: `Endpoint Class Routing Behavior` requirement keeps the
    routing matrix restricted to `agent` and `ui`; operator does not become a
    delivery target.

- **Policy preset complexity.** Operator-class claim must be wired to the
  policy preset evaluation path at hello time.
  - Mitigation: failure code (`validation_invalid_client_class_for_hello`)
    is fail-fast; mis-configuration cannot silently grant decision authority.

- **MCP polling load on relay.** Repeated `grant list` calls cost JSON
  reads under the queue mutex.
  - Mitigation: callers expected to poll on demand, not continuously; queue
    file is small (bounded `max_pending`).

## Migration Notes

- Default `client_class=operator` preset is opt-in. Existing bundles continue
  to operate unchanged with TUI-only decisioning.
- No data-model changes to `permission_queue.json` schema.
- TUI behavior unchanged; UI-class push event channel unchanged.

## Open for Implementation-Phase Review

- Exact serde representation of `client_class=operator` (likely
  `#[serde(rename = "operator")]` matching existing snake_case style).
- Whether operator-class registration emits a distinct inscription tag for
  observability of the new class.
