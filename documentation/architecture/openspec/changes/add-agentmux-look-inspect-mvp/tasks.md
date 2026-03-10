## 1. Implementation

- [x] 1.1 Add relay `look` request/response handling with canonical payload
      fields and `snapshot_lines: string[]`.
- [x] 1.2 Enforce look defaults and validation:
      - same-bundle-only scope in MVP,
      - lines default/max bounds (`120`, `1000`),
      - stable structured error codes.
- [ ] 1.3 Add CLI `agentmux look <target-session>` with optional
      `--bundle` and `--lines` flags and default JSON output.
- [x] 1.4 Add MCP `look` tool mapped to relay `look` with schema parity.
- [ ] 1.5 Ensure MCP and CLI surfaces share aligned error taxonomy and payload
      semantics.
- [x] 1.6 Keep naming semantics aligned with `mcp/11` direction (`send` for
      delivery, `look` for inspection) and update follow-up tracker references
      if contract sync scope changes.
- [x] 1.7 Track authorization-policy behavior as out-of-scope in this change
      and capture explicit follow-up spec linkage.

## 2. Testing

- [ ] 2.1 Add CLI integration tests for:
      - successful look response shape,
      - same-bundle scope enforcement,
      - invalid lines bounds.
- [x] 2.2 Add MCP integration tests for:
      - tool catalog includes `look`,
      - successful look response shape,
      - validation errors.
- [x] 2.3 Add relay/runtime tests for:
      - same-bundle scope enforcement,
      - optional/redundant `bundle_name` request behavior,
      - canonical line ordering in `snapshot_lines`.

## 3. Validation

- [x] 3.1 Run `openspec validate add-agentmux-look-inspect-mvp --strict`.
- [x] 3.2 Run `cargo check --all-targets --all-features`.
- [x] 3.3 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 3.4 Run `cargo test --all-features`.
