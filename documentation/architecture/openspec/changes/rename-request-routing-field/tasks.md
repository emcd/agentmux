## Pre-implementation decisions (open)

- [ ] P.1 Confirm: mixed targets (bundle + relay-wide) in one send — not
      supported (separate sends required)? See design.md open questions.
- [ ] P.2 Confirm: broadcast under GLOBAL namespace — out of scope for this
      change?
- [ ] P.3 Confirm: error code name — `validation_missing_routing_namespace` or
      keep `validation_missing_target_bundle`?
- [ ] P.4 Confirm: `EXTERNAL` and `RELAY` namespace routing — reserved but
      unimplemented in this slice?

## 1. Relay (primary)

- [ ] 1.1 Rename `bundle_name` → `namespace` in `IncomingEnvelope` and
      `IncomingFrame` in `src/relay/stream.rs`.
- [ ] 1.2 Rename corresponding field in `src/relay/client.rs` request frame.
- [ ] 1.3 Update `resolve_effective_bundle` in `src/relay/connection.rs`:
      accept relay-wide namespace specifiers (`"GLOBAL"`, `"EXTERNAL"`,
      `"RELAY"`) in addition to bundle names; route to relay-wide registry for
      specifiers; route to bundle catalog for bundle names.
- [ ] 1.4 Update all call sites that pass `bundle_name` in requests (CLI,
      tests).
- [ ] 1.5 Rename or update error code per P.3 decision.

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
