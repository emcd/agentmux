## Pre-implementation decisions (resolved)

- [x] P.1 One namespace per request in this slice. Cross-namespace fan-out
      (mixed registries in one send, e.g. bundle + @GLOBAL targets together)
      deferred to `cross-namespace-routing` proposal (designs/relay/6). The
      `namespace` field design does not preclude per-target derivation later.
      (D4)
- [x] P.2 Broadcast under `namespace = "GLOBAL"` is out of scope for this
      change. Relay-wide broadcast semantics filed for separate design review.
      (D5)
- [x] P.3 Error code: rename `validation_missing_target_bundle` →
      `validation_missing_routing_namespace`. (D, task 1.5)
- [x] P.4 `EXTERNAL` and `RELAY`: accept and parse as valid namespace values;
      return `validation_unsupported_namespace` if a client attempts direct
      routing. Only the relay routes to these namespaces under defined protocol
      circumstances. (D6)

## 1. Relay (primary)

- [x] 1.1 Rename `bundle_name` → `namespace` in `IncomingEnvelope` and
      `IncomingFrame` in `src/relay/stream.rs`. Scope: the routing context
      selector only. Per-variant `bundle_name` fields inside `RelayRequest`
      variants (e.g., `PermissionList`) are NOT renamed here. (D4)
- [x] 1.2 Rename corresponding routing selector field in
      `src/relay/client.rs` request frame. Same scope constraint as 1.1.
- [x] 1.3 Update `resolve_effective_bundle` in `src/relay/connection.rs`:
      accept relay-wide namespace specifiers (`"GLOBAL"`, `"EXTERNAL"`,
      `"RELAY"`) in addition to bundle names; return
      `validation_unsupported_namespace` for `"EXTERNAL"` and `"RELAY"`
      (reserved — only relay routes to these); route to bundle catalog for
      bundle names; absent + bound → bound; absent + relay-wide →
      `validation_missing_routing_namespace`.
      NOTE (this slice): `"GLOBAL"` routing-to-relay-wide-registry is a new
      delivery feature, not a rename — it needs a non-bundle dispatch path plus
      relay-wide send authorization, is unreachable without MCP task 2.2, and is
      untestable without tests 3.1-3.3. It is therefore STUBBED here: `"GLOBAL"`
      returns the distinct, temporary `validation_namespace_routing_unavailable`
      (rather than a misleading catalog miss). The relay-wide delivery path that
      flips `"GLOBAL"` from reject to route is deferred to a follow-up slice that
      lands together with MCP task 2.2 and tests 3.1-3.3. (Coordinator-approved
      scope split, 2026-05-30.)
- [x] 1.4 Update all call sites that pass `bundle_name` in requests (CLI,
      tests). The client request frame is populated internally from the bound
      bundle (`client.rs`); the only test-built frame selectors were the
      `relay_stream/permissions.rs` request frames, now sent under `namespace`.
- [x] 1.5 Rename error code `validation_missing_target_bundle` →
      `validation_missing_routing_namespace` in relay contract and all
      call sites (tests, MCP, TUI). Only `connection.rs` referenced the code;
      no MCP/TUI/test call sites existed.

## 2. MCP

- [x] 2.1 Rename `bundle_name` parameter to `namespace` in `look` and `raww`
      MCP tool schemas. (Completed; then corrected by add-global-namespace-routing
      2.1 which removed `namespace` from these tools entirely.)
- [x] 2.2 Add optional `namespace` parameter to `send` MCP tool.
      (Completed; then corrected by add-global-namespace-routing 2.2 which
      removed `namespace` from `send` — design error identified during review.)
- [x] 2.3 Update MCP tool dispatch to pass `namespace` through.
      (Completed; then collapsed back by add-global-namespace-routing 2.3.)
- [x] 2.4 Update MCP inventory tests for changed tool schemas.
      (Completed; updated again by add-global-namespace-routing 2.4.)

## 3. Tests

- [x] 3.1 Integration test: session principal in bundle `A` sends with
      `targets = ["operator@GLOBAL"]` → message delivered to registered `@GLOBAL`
      UI session. Covered by add-global-namespace-routing
      (`routing.rs::send_to_global_target_is_delivered_to_registered_operator`).
- [x] 3.2 Integration test: relay-wide `@GLOBAL` principal sends with a
      bundle-qualified target → message delivered. Covered by add-global-namespace-routing
      (`routing.rs::relay_wide_send_routes_to_bundle_target_by_suffix`).
- [x] 3.3 Integration test: relay-wide principal without namespace / bare targets →
      `validation_missing_routing_namespace`. Covered by add-global-namespace-routing
      (`routing.rs::relay_wide_send_with_bare_target_is_rejected`).
- [x] 3.4 Integration test: existing bundle-scoped send flow unchanged after
      field rename. Covered by existing integration harness, which now runs
      all bundle-scoped send/look tests under the renamed `namespace` field.
