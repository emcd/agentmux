## 1. Implementation (landed in 7f6585a)

- [x] 1.1 Resolve look target bundle from `target_session` suffix
  (`resolve_look_target_bundle`); capture from the peer bundle's runtime.
- [x] 1.2 Replace the cross-bundle rejection with peer resolution; emit
  `validation_unknown_bundle` / `validation_unknown_target`.
- [x] 1.3 Gate cross-bundle look on `all:all` (`authorize_look` `cross_bundle`
  flag); keep `all:home` for same-bundle, self-shortcut same-bundle only.
- [x] 1.4 Cross-bundle look stream tests (permitted / denied-under-home /
  unknown-bundle / unknown-target); update unit + MCP mapping tests.
- [x] 1.5 Update `src/relay/README.md` and `src/tui/README.md`.

## 2. Spec reconciliation (this change)

- [x] 2.1 `session-relay` `Relay Look Operation` delta.
- [x] 2.2 `cli-surface` `Look Command Surface` companion delta.
- [x] 2.3 `mcp-tool-surface` `MCP Look Tool` companion delta.
- [ ] 2.4 Deferred (separate work): reconcile `Same-Bundle Stream Scope
  Enforcement` wording with suffix-based cross-bundle routing (with the
  `add-cross-namespace-routing` archive, or the routing/authz-layer work).
