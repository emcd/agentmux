# Change: Retire bundle_name from request-side parameters

## Why

Three `RelayRequest` variants still carry `bundle_name: Option<String>` as a
caller-supplied routing selector — a vestige of pre-canonical-id addressing:

- `PermissionResolve.bundle_name` / `PermissionList.bundle_name`: used only to
  cross-check that the caller is not accidentally targeting a different bundle.
  The session is always associated with exactly one bundle; the check is
  redundant and the field adds noise to the wire format.
- `IdentityIntrospect.bundle_name`: used to qualify a potentially bare
  `target_session` into canonical `session@bundle` form. Now that the
  "Canonical Session Identity" requirement mandates canonical ids throughout,
  callers are expected to supply qualified ids and this helper field is no
  longer needed.

## What Changes

- **BREAKING** Remove `bundle_name` from `RelayRequest::PermissionResolve`.
  Same-bundle scope is enforced implicitly by the session context.
- **BREAKING** Remove `bundle_name` from `RelayRequest::PermissionList`.
  Same-bundle scope is enforced implicitly by the session context.
- **BREAKING** Remove `bundle_name` from `RelayRequest::IdentityIntrospect`.
  `target_session` MUST be supplied as a qualified principal id
  (`<id>@<namespace>`); a bare (unqualified) id is rejected with
  `validation_invalid_params`.
- Remove the bundle-name cross-check from `handle_permission_list` and
  `validate_permission_decision_request`; the `validation_cross_bundle_unsupported`
  path for these operations is no longer reachable.
- Retire `validation_cross_bundle_unsupported` from the `mcp-tool-surface`
  `grant` passthrough taxonomy and the `tui-surface` raww error taxonomy; also
  removes the stale "Same-Bundle Stream Scope Enforcement" requirement from
  `session-relay`.

No change to response formats, event payloads, or the MCP/CLI tool surface
wire shape beyond field removals in callers.

## Impact

- Affected specs: `session-relay`, `relay-identity`, `mcp-tool-surface`,
  `tui-surface`
- Affected code: `src/relay/contract.rs`, `src/relay/handlers/permissions.rs`,
  `src/relay/handlers/identity.rs`, `src/mcp/server/handlers/grant.rs` (AE),
  TUI permission flows in `src/tui/` (FE)
- Cross-lane: relay (BE), MCP (AE), TUI (FE)
