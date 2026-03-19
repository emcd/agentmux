## 1. Policy Schema and Binding

- [x] 1.1 Add policy preset loader for `<config-root>/policies.toml` with
      `[[policies]]` entries (`id`, optional `description`, required
      `[controls]`).
- [x] 1.4 Add optional top-level `default = "<policy-id>"` handling and
      conservative built-in default fallback when absent.
- [x] 1.2 Add session-level policy binding (`policy = "<policy-id>"`) in
      bundle session schema.
- [x] 1.2a Implement session policy resolution precedence:
      explicit session policy -> top-level default preset -> conservative
      built-in default policy.
- [x] 1.3 Enforce fail-fast behavior for missing/invalid policy artifact and
      unknown session policy references.

## 2. Relay Authorization Engine

- [x] 2.1 Implement centralized relay authorization decisioning for current
      MVP relay operations (`list`, `look`, `send`) using canonical policy
      controls/scopes.
- [x] 2.5a Treat `do` scopes `all:home` and `all:all` as reserved/non-operative
      while `do` remains self-target-only in MVP.
- [x] 2.2 Enforce validation-first ordering before authorization checks.
- [x] 2.3 Emit `authorization_forbidden` with locked minimum details schema.
- [x] 2.4 Enforce MVP posture locks:
      - `look` default self-only
      - cross-bundle `look` remains unsupported by runtime contract
      - default `send` scope `all:home`, cross-bundle via explicit `all:all`

## 3. Surface Integration (Adapter-Only)

- [x] 3.1 Ensure MCP remains validator/adapter only and performs no shadow
      authorization decisioning.
- [x] 3.2 Ensure CLI remains validator/adapter only and performs no shadow
      authorization decisioning.
- [x] 3.3 Preserve relay-authored denial payloads unchanged across surfaces.

## 4. Tests

- [x] 4.1 Add policy loader tests for missing/invalid `policies.toml`.
- [x] 4.2 Add config validation tests for unknown session `policy` id.
- [x] 4.4 Add relay tests for `look` self-only default and explicit broader
      scope behavior.
- [x] 4.6 Add MCP/CLI integration tests confirming adapter-only behavior and
      canonical denial payload propagation.

## Deferred Follow-ups (Out of Current Runtime Support)

- `do` execution-time authorization semantics (`none` + missing-entry default)
  are deferred to `add-do-action-tool-mvp` where `do` runtime execution exists.
- `send` transition tests for `all:home` vs `all:all` are deferred until
  cross-bundle send targeting is implemented in runtime request/target shape.

## 5. Validation

- [x] 5.1 Run `openspec validate add-authorization-policy-mvp --strict`.
- [x] 5.2 Run `cargo check --all-targets --all-features`.
- [x] 5.3 Run `cargo test --all-features --test integration mcp_tool_surface`.
- [x] 5.4 Run `cargo test --all-features --test integration cli_surface`.
- [x] 5.5 Run `cargo test --all-features --test integration session_relay_delivery`.
