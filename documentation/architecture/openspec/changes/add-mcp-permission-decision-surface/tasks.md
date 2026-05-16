## 1. Client Class Taxonomy (Contract)

- [x] 1.1 Lock `RelayClientClass` vocabulary as `{agent, ui, operator}` and
      define operator class as decision-submitter-only (not a delivery
      target, not a push-event recipient).
- [x] 1.2 Lock hello-frame `client_class` vocabulary expansion to include
      `operator` and define `validation_invalid_client_class_for_hello`
      taxonomy entry for unauthorized claims.
- [x] 1.3 Lock operator-class authorization at hello time via bundle policy
      preset; reject unauthorized claims fail-fast.
- [x] 1.4 Lock endpoint-class routing matrix: operator is delivery-inert in
      alpha; routing-target resolution for operator is rejected.

## 2. Decision Gate Relaxation (Contract)

- [x] 2.1 Rename `UI-Mediated Decision Submitter Gate` to
      `Permission Decision Submitter Gate` and relax submitter class to
      `{ui, operator}`; continue to reject `agent` with
      `validation_invalid_client_class_for_action`.
- [x] 2.2 Lock `authorize_grant` policy check as binding and independent of
      submitter class.
- [x] 2.3 Lock arbitration semantics: first authorized `{ui, operator}`
      decision wins.
- [x] 2.4 Lock denial-schema uniformity across `{ui, operator}` submitters;
      canonical denial fields unchanged.

## 3. Permission Polling Visibility (Contract)

- [x] 3.1 Lock `RelayRequest::PermissionList` contract: requires associated
      principal with `client_class ∈ {ui, operator}` and `grant` capability.
- [x] 3.2 Lock `PermissionList` response payload field set to mirror
      `permission.requested` event payload exactly.
- [x] 3.3 Lock same-bundle scope rejection
      (`validation_cross_bundle_unsupported`).
- [x] 3.4 Lock push-events-remain-UI-only invariant for alpha; operator
      visibility is poll-only.

## 4. MCP Tool Surface (Contract)

- [x] 4.1 Lock top-level MCP tool `grant` advertisement with subcommand
      selector `command ∈ {list, resolve}`.
- [x] 4.2 Lock `grant list` argument schema (optional `bundle_name`
      only; unknown fields rejected with `validation_invalid_params`).
- [x] 4.3 Lock `grant resolve` argument schema:
      `permission_request_id`, `outcome ∈ {selected, cancelled}`,
      `option_id` required-iff-selected, unknown fields rejected.
- [x] 4.4 Lock rejection of sender-like identity fields (`decided_by`,
      `ui_session_id`, `operator_session_id`) with
      `validation_invalid_params`.
- [x] 4.5 Lock MCP sender authority as association-derived; relay-derived
      `decided_by` echoed unchanged.
- [x] 4.6 Lock relay error taxonomy passthrough for `grant` tool
      including all permission-related codes.
- [x] 4.7 Lock `help` query catalog entries for `grant`,
      `grant.list`, `grant.resolve`.

## 5. Implementation Follow-up (post-approval)

- [x] 5.1 Extend `RelayClientClass` enum to include `Operator` with
      `#[serde(rename = "operator")]`; update all match arms across relay
      modules.
- [x] 5.2 Implement hello-time operator-class authorization against the
      bundle policy preset; add
      `validation_invalid_client_class_for_hello` error path.
- [x] 5.3 Implement `RelayRequest::PermissionList` request variant and
      handler reusing `list_pending_permission_requests`; lock response
      payload shape to match the spec.
- [x] 5.4 Relax `permission.resolve` submitter gate to accept
      `client_class ∈ {ui, operator}`.
- [x] 5.5 Wire operator-class authorization in the bundled operator policy
      preset so `master` and `mcp` lanes may claim operator; other lanes
      default to agent.
- [x] 5.6 Register MCP server stream as `client_class=operator` when the
      runtime configuration permits; fall back to `agent` cleanly when
      operator authorization is not granted.
- [x] 5.7 Implement MCP `grant` tool with `list` and `resolve`
      subcommands, argument validation, help catalog entries, and relay
      passthrough.
- [x] 5.8 Add integration coverage for:
      - operator-class hello accept/reject paths,
      - `PermissionList` shape parity with `permission.requested`,
      - decision gate accept (ui + operator), reject (agent),
      - sender-authority rejection of payload identity fields,
      - cross-bundle rejection,
      - already-resolved idempotency,
      - error-taxonomy passthrough end-to-end.

## 6. Documentation

- [x] 6.1 Update `src/mcp/README.md` to advertise the new `grant` tool.
- [x] 6.2 Document operator-class assignment policy (which sessions default
      to operator) in the bundle configuration documentation.

## 7. Validation

- [x] 7.1 Run `openspec validate add-mcp-permission-decision-surface --strict`.
