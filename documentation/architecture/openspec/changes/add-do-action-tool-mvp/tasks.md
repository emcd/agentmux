## 1. Implementation

- [ ] 1.1 Add configuration model for per-coder do-action entries from
      `coders.toml` (kebab-case TOML keys), with metadata:
      - canonical path: `[[coders.do-actions]]`
      - action id
      - prompt/template payload
      - optional description
      - `self-only` flag (default `true`)
- [ ] 1.2 Add relay action dispatch operation for configured actions
      (`do list`, `do show`, `do run`) with stable response schema.
- [ ] 1.3 Add CLI surface:
      - `agentmux do` (list actions)
      - `agentmux do --show <action>` (show action metadata)
      - `agentmux do <action>` (execute action)
- [ ] 1.4 Add MCP `do` tool with mode-based request schema:
      - `list`
      - `show`
      - `run`
      - reject target selector fields in MVP
- [ ] 1.5 Enforce safety semantics:
      - reject unconfigured action id
      - enforce self-target-only run semantics in MVP
      - force async execution for self-target runs
      - defer broader authorization policy to follow-up authorization work
- [ ] 1.6 Lock canonical acceptance payload for `do run` across relay/CLI/MCP:
      - `schema_version`
      - `bundle_name`
      - `requester_session`
      - `action`
      - `status=accepted`
      - `outcome=queued`
      - `message_id`
- [ ] 1.7 Add inscriptions for `do` lifecycle:
      - request accepted
      - queued
      - delivered
      - timeout/failed

## 2. Testing

- [ ] 2.1 Add CLI integration tests for:
      - listing actions
      - showing action metadata
      - running configured action
      - rejecting unknown action
      - rejecting target selector arguments in MVP
      - canonical run acceptance payload fields
- [ ] 2.2 Add MCP integration tests for:
      - `do` tool visibility
      - `list`/`show` response shape
      - `run` response shape
      - validation errors (including disallowed target selector fields)
      - canonical run acceptance payload fields
- [ ] 2.3 Add relay/runtime tests for:
      - configured action lookup
      - forced async for self-run
      - rejecting target selector fields in MVP
      - canonical run acceptance payload fields

## 3. Validation

- [ ] 3.1 Run `openspec validate add-do-action-tool-mvp --strict`.
- [ ] 3.2 Run `cargo check --all-targets --all-features`.
- [ ] 3.3 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] 3.4 Run `cargo test --all-features`.
