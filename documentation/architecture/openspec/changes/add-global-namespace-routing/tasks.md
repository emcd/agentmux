## 1. Relay

- [ ] 1.1 Extend `handle_send` in `src/relay/handlers.rs` to resolve each
      target via `registry_key_for_target`: `RegistryKey::RelayWide` targets
      look up directly in the stream registry; `RegistryKey::Session` targets
      use the existing bundle-member path. If a single Send mixes relay-wide
      and session targets, return `validation_conflicting_namespaces` (D3).
- [ ] 1.2 Remove the GLOBAL stub from `resolve_effective_bundle` in
      `src/relay/connection.rs`; relay no longer routes based on the wire
      `namespace` field for Send/Look/Raww — suffix inference handles it.
      `resolve_effective_bundle` retains its role for `List` and for the
      bundle-name case on relay-wide senders.
- [ ] 1.3 Implement `handle_global_list` in `src/relay/handlers.rs`: when
      `namespace = "GLOBAL"` on a `List` request, return all currently
      registered relay-wide sessions (`RegistryKey::RelayWide` entries).
      Resolves `todos/relay/61`; unblocks `todos/mcp/30`.
- [ ] 1.4 Remove `validation_namespace_routing_unavailable` from the relay
      error contract. Verify no remaining call sites.
- [ ] 1.5 Add `validation_conflicting_namespaces` to the relay error contract
      (returned when a Send mixes relay-wide and session targets, per D3).

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

- [ ] 3.1 Integration test (rename tasks 3.1): bundle-bound session sends with
      `targets = ["operator@GLOBAL"]` → message delivered to the registered
      `@GLOBAL` UI session. No `namespace` field required.
- [ ] 3.2 Integration test (rename tasks 3.2): `@GLOBAL` principal sends with
      `targets = ["session@bundle"]` → message delivered to that bundle session.
- [ ] 3.3 Integration test (rename tasks 3.3): relay-wide principal sends with
      bare targets (no suffix, no bound bundle) →
      `validation_missing_routing_namespace`.
- [ ] 3.4 Integration test: `List` with `namespace = "GLOBAL"` → returns
      registered relay-wide sessions; confirms `todos/relay/61` contract.
- [ ] 3.5 Integration test: Send mixing `@GLOBAL` and `@<bundle>` targets in
      one request → `validation_conflicting_namespaces`.
- [ ] 3.6 Unit test: `validation_namespace_routing_unavailable` no longer
      appears in routing output (regression guard on stub retirement).

## 4. Spec / tracking

- [ ] 4.1 Mark `todos/relay/61` done after task 1.3 is validated.
- [ ] 4.2 Mark rename tasks 3.1–3.3 done after tests 3.1–3.3 pass.
