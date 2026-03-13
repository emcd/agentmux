## 1. Policy Schema and Binding

- [ ] 1.1 Add policy preset loader for `<config-root>/policies.toml` with
      `[[policies]]` entries (`id`, optional `description`, required
      `[controls]`).
- [ ] 1.4 Add optional top-level `default = "<policy-id>"` handling and
      conservative built-in default fallback when absent.
- [ ] 1.2 Add session-level policy binding (`policy = "<policy-id>"`) in
      bundle session schema.
- [ ] 1.2a Implement session policy resolution precedence:
      explicit session policy -> top-level default preset -> conservative
      built-in default policy.
- [ ] 1.3 Enforce fail-fast behavior for missing/invalid policy artifact and
      unknown session policy references.

## 2. Relay Authorization Engine

- [ ] 2.1 Implement centralized relay authorization decisioning using controls:
      - `find`, `list`, `look`, `send`, `do`
      with scopes `self`, `all:home`, `all:all`.
- [ ] 2.5 Implement `do` control semantics for `none` and missing-entry =>
      `none` default.
- [ ] 2.5a Treat `do` scopes `all:home` and `all:all` as reserved/non-operative
      while `do` remains self-target-only in MVP.
- [ ] 2.2 Enforce validation-first ordering before authorization checks.
- [ ] 2.3 Emit `authorization_forbidden` with locked minimum details schema.
- [ ] 2.4 Enforce MVP posture locks:
      - `look` default self-only
      - cross-bundle `look` remains unsupported by runtime contract
      - default `send` scope `all:home`, cross-bundle via explicit `all:all`

## 3. Surface Integration (Adapter-Only)

- [ ] 3.1 Ensure MCP remains validator/adapter only and performs no shadow
      authorization decisioning.
- [ ] 3.2 Ensure CLI remains validator/adapter only and performs no shadow
      authorization decisioning.
- [ ] 3.3 Preserve relay-authored denial payloads unchanged across surfaces.

## 4. Tests

- [ ] 4.1 Add policy loader tests for missing/invalid `policies.toml`.
- [ ] 4.2 Add config validation tests for unknown session `policy` id.
- [ ] 4.3 Add relay tests for `list` deny semantics (no empty-success fallback).
- [ ] 4.4 Add relay tests for `look` self-only default and explicit broader
      scope behavior.
- [ ] 4.5 Add relay tests for send scope transitions (`all:home` vs `all:all`).
- [ ] 4.6 Add MCP/CLI integration tests confirming adapter-only behavior and
      canonical denial payload propagation.

## 5. Validation

- [ ] 5.1 Run `openspec validate add-authorization-policy-mvp --strict`.
- [ ] 5.2 Run `cargo check --all-targets --all-features`.
- [ ] 5.3 Run `cargo test --all-features --test integration mcp_tool_surface`.
- [ ] 5.4 Run `cargo test --all-features --test integration cli_surface`.
- [ ] 5.5 Run `cargo test --all-features --test integration session_relay_delivery`.
