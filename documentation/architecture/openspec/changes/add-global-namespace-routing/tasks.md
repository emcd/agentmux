## 1. Relay

- [x] 1.1 Extend `handle_send` in `src/relay/handlers.rs` to resolve each
      target via `registry_key_for_target`: `RegistryKey::RelayWide` targets
      look up directly in the stream registry; `RegistryKey::Session` targets
      use the existing bundle-member path. If a single Send mixes relay-wide
      and session targets, return `validation_conflicting_namespaces` (D3).
      NOTE: `@GLOBAL` target delivery already flows through the existing
      relay-wide-UI delivery path (`send_event_to_registered_ui` re-derives the
      `RegistryKey::RelayWide` key, and relay-wide operators are visible in every
      bundle's authorization context via the relay-wide `users.toml`). The only
      new `handle_send` logic is the conflicting-namespaces guard on normalized
      targets; delivery wiring needed no change.
- [x] 1.2 Remove the GLOBAL stub from `resolve_effective_bundle` in
      `src/relay/connection.rs`; relay no longer routes based on the wire
      `namespace` field for Send — suffix inference handles it. `Send` now
      derives its routing bundle from target suffixes (session sender → bound
      bundle; relay-wide sender → first `@<bundle>` target, else
      `validation_missing_routing_namespace`). `List`/`Up`/`Down`/`Permission*`
      retain wire-`namespace` routing (`resolve_namespace_routing_bundle`,
      `EXTERNAL`/`RELAY` still reserved). `List` with `namespace = "GLOBAL"` is
      intercepted at the connection layer and dispatched to `handle_global_list`
      (no bundle context).
- [x] 1.3 Implement `handle_global_list` in `src/relay/handlers.rs`: when
      `namespace = "GLOBAL"` on a `List` request, return all currently
      registered relay-wide sessions (`RegistryKey::RelayWide` entries) via the
      new `stream::list_registered_relay_wide_sessions` accessor, shaped as a
      `RelayResponse::List` over a synthetic `GLOBAL` bundle view.
      Resolves `todos/relay/61`; unblocks `todos/mcp/30`.
- [x] 1.4 Remove `validation_namespace_routing_unavailable` from the relay
      error contract. Verified no remaining emit sites in `src/` (only the
      task 3.6 regression guards in tests reference the literal, asserting it is
      absent). No central code catalog exists; the spec delta documents removal.
- [x] 1.5 Add `validation_conflicting_namespaces` to the relay error contract
      (returned when a Send mixes relay-wide and session targets, per D3). Emitted
      from `handle_send`; documented in the session-relay spec delta.

## 2. MCP

- [ ] 2.1 Remove `namespace` field from `LookParams` and `RawwParams` in
      `src/mcp/params.rs`. Update tool schema in `src/mcp/help.rs` and
      validation in `src/mcp/validation.rs`.
- [ ] 2.2 Remove `namespace` field from `SendParams` in `src/mcp/params.rs`.
      Update tool schema and validation accordingly.
- [ ] 2.3 Remove the `request_with_namespace_and_events` entry point added in
      `rename-request-routing-field` task 2.2 from `src/relay/client.rs` (or
      `src/mcp/server.rs`) if it is no longer needed once `namespace` is
      removed from Send/Look/Raww dispatch.
- [ ] 2.4 Update MCP inventory and envelope tests to reflect removed `namespace`
      parameters on `send`, `look`, and `raww`.

## 3. Tests

- [x] 3.1 Test (rename tasks 3.1): bundle-bound session sends with
      `targets = ["operator@GLOBAL"]` → message delivered to the registered
      `@GLOBAL` UI session. No `namespace` field required.
      (`tests/unit/relay_stream/routing.rs::send_to_global_target_is_delivered_to_registered_operator`,
      plus the pre-existing
      `tests/integration/session_relay_stream.rs::relay_send_routes_to_connected_ui_stream_with_event_frames`.)
- [x] 3.2 Test (rename tasks 3.2): `@GLOBAL` principal sends with
      `targets = ["session@bundle"]` → relay routes to that bundle and resolves
      the target.
      (`tests/unit/relay_stream/routing.rs::relay_wide_send_routes_to_bundle_target_by_suffix`.)
- [x] 3.3 Test (rename tasks 3.3): relay-wide principal sends with bare targets
      (no suffix, no bound bundle) → `validation_missing_routing_namespace`.
      (`tests/unit/relay_stream/routing.rs::relay_wide_send_with_bare_target_is_rejected`.)
- [x] 3.4 Test: `List` with `namespace = "GLOBAL"` → returns registered
      relay-wide sessions (and excludes bundle sessions); confirms
      `todos/relay/61` contract.
      (`tests/unit/relay_stream/routing.rs::list_global_namespace_returns_registered_relay_wide_sessions`
      and `::list_global_namespace_excludes_bundle_sessions`.)
- [x] 3.5 Test: Send mixing `@GLOBAL` and `@<bundle>` targets in one request →
      `validation_conflicting_namespaces`.
      (`tests/unit/relay_stream/routing.rs::send_mixing_relay_wide_and_session_targets_is_rejected`.)
- [x] 3.6 Test: `validation_namespace_routing_unavailable` no longer appears in
      routing output (regression guard on stub retirement, covering both the
      `List`+`GLOBAL` and `@GLOBAL`-target `Send` paths).
      (`tests/unit/relay_stream/routing.rs::global_routing_no_longer_returns_unavailable_stub`.)

## 4. Spec / tracking

- [x] 4.1 Mark `todos/relay/61` done after task 1.3 is validated.
- [x] 4.2 Mark rename tasks 3.1–3.3 done after tests 3.1–3.3 pass.
      NOTE: the `rename-request-routing-field` proposal is owned by the
      Coordinator's archive/spec-merge lane; its `tasks.md` 3.1–3.3 checkboxes
      are flagged for the Coordinator to mark when archiving, rather than edited
      cross-proposal from this lane.
