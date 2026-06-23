## 1. Registry Model

- [ ] 1.1 Replace `RegistryKey` with canonical `principal_id` string keys in
      `src/relay/stream.rs`.
- [ ] 1.2 Add a unified registry entry shape carrying namespace, bare session id,
      principal class, registration source, transport/runtime binding,
      readiness state, authenticated identity, revoke signal, writer, and
      transport capability flags.
- [ ] 1.3 Add canonical-principal helper functions that fail fast for unqualified
      or non-canonical registry keys.

## 2. Registration And Lifecycle

- [ ] 2.1 Register static bundle-runtime coder entries during startup/reconcile,
      including runtime directory, transport binding, session type, readiness
      state, and capability flags.
- [ ] 2.2 Update stream hello registration to create or attach dynamic stream
      state for bundle sessions and relay-wide sessions.
- [ ] 2.3 Update bundle unload/reload eviction to filter entries by namespace
      rather than by `RegistryKey::Session`.
- [ ] 2.4 Update credential revocation and identity-event fan-out to scan unified
      entries by authenticated identity and principal class.
- [ ] 2.5 Preserve identity-claim collision behavior for reconnects to the same
      canonical `principal_id`.

## 3. Operation Integration

- [ ] 3.1 Replace `handle_global_list` in
      `src/relay/handlers/dispatch.rs` and
      `list_registered_relay_wide_sessions` in `src/relay/stream.rs` with
      namespace-filtered list handling over the unified registry.
- [ ] 3.2 Update send delivery assembly to resolve target registry entries by
      canonical `principal_id` and derive coder-vs-stream delivery from each
      entry's transport binding instead of carrying relay-wide target flags.
- [ ] 3.3 Update look and raww preparation to read target capabilities from the
      unified entry while preserving capability-before-authorization precedence
      and current unavailable/stale behavior for configured-but-not-ready coder
      targets.
- [ ] 3.4 Remove `resolve_relay_wide_target_session_type` and
      `relay_wide_operation_unimplemented` from `src/relay/handlers/routed.rs`
      once operation support is determined from entry capabilities.
- [ ] 3.5 Update `src/relay/delivery/async_worker.rs`,
      `src/relay/delivery/dispatch/worker.rs`, and
      `src/relay/delivery/dispatch/payload.rs` to consume registry-provided
      transport/runtime binding for coder and stream-delivered targets.

## 4. Cleanup

- [ ] 4.1 Remove `RegistryKey::RelayWide`, `RegistryKey::Session`, and helpers
      that exist only to translate between the two key shapes.
- [ ] 4.2 Remove `relay_wide_target` and `ResolvedTarget.relay_wide` fields after
      delivery and payload paths derive stream-delivered versus coder-delivered
      behavior from registry entry transport binding.
- [ ] 4.3 Update `src/relay/README.md` and relevant module comments to describe
      the unified namespace-keyed registry.

## 5. Tests And Validation

- [ ] 5.1 Add/update registry unit tests for canonical keying, duplicate
      registration rejection, namespace-filtered enumeration, and lifecycle
      eviction.
- [ ] 5.2 Add/update integration tests for `GLOBAL` list, bundle list, send to
      `@GLOBAL`, send to bundle sessions, look capability rejection, and raww
      capability rejection.
- [ ] 5.3 Run `openspec validate refactor-unified-namespace-registry --strict`.
- [ ] 5.4 Run `cargo fmt --check`.
- [ ] 5.5 Run `cargo clippy -- -D warnings`.
- [ ] 5.6 Run targeted relay registry/routing tests and `cargo test` if targeted
      validation does not cover changed paths.
