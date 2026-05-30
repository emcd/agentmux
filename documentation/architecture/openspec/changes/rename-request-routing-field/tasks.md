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

- [ ] 1.1 Rename `bundle_name` → `namespace` in `IncomingEnvelope` and
      `IncomingFrame` in `src/relay/stream.rs`.
- [ ] 1.2 Rename corresponding field in `src/relay/client.rs` request frame.
- [ ] 1.3 Update `resolve_effective_bundle` in `src/relay/connection.rs`:
      accept relay-wide namespace specifiers (`"GLOBAL"`, `"EXTERNAL"`,
      `"RELAY"`) in addition to bundle names; route to relay-wide registry for
      `"GLOBAL"`; return `validation_unsupported_namespace` for `"EXTERNAL"` and
      `"RELAY"` (reserved — only relay routes to these); route to bundle catalog
      for bundle names.
- [ ] 1.4 Update all call sites that pass `bundle_name` in requests (CLI,
      tests).
- [ ] 1.5 Rename error code `validation_missing_target_bundle` →
      `validation_missing_routing_namespace` in relay contract and all
      call sites (tests, MCP, TUI).

## 2. MCP

- [ ] 2.1 Rename `bundle_name` parameter to `namespace` in `look` and `raww`
      MCP tool schemas.
- [ ] 2.2 Add optional `namespace` parameter to `send` MCP tool to enable
      `@GLOBAL` targeting from bundle-bound sessions.
- [ ] 2.3 Update MCP tool dispatch to pass `namespace` through to relay request
      envelope.
- [ ] 2.4 Update MCP inventory tests for changed tool schemas.

## 3. Tests

- [ ] 3.1 Integration test: session principal in bundle `A` sends with
      `namespace = "GLOBAL"` → message delivered to registered `@GLOBAL`
      UI session.
- [ ] 3.2 Integration test: relay-wide `@GLOBAL` principal sends with
      `namespace = "<bundle>"` → message delivered to sessions in that bundle.
- [ ] 3.3 Integration test: relay-wide principal without `namespace` →
      typed error returned.
- [ ] 3.4 Integration test: existing bundle-scoped send flow unchanged after
      field rename.
