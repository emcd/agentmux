## 0. Prerequisite

- [ ] 0.1 Archive `add-mcp-permission-decision-surface` so grant tool
  requirements land in the base `mcp-tool-surface` spec before this change's
  MODIFIED delta applies

## 1. Config layer (coordinator — `src/configuration.rs`, `src/runtime/**`)

- [x] 1.1 `[[sessions]]` schema: a session is coder-backed (flat `coder` +
  optional `coder-session-id`) or coder-less (exactly one `[sessions.ui]` or
  `[sessions.pubsub]` marker subtable); reject zero or multiple shapes, and
  reject `coder-session-id` on a coder-less session, with structured config
  errors
- [x] 1.2 Transport derivation: a coder-backed session's transport (tmux vs
  acp) is derived from the referenced coder's descriptor. The session does
  not restate the transport, so no session-coder consistency check is needed
- [x] 1.3 `[sessions.ui]` and `[sessions.pubsub]`: parse and validate (empty
  body is valid); at startup emit `runtime_session_type_not_implemented` and
  exclude from active routing rather than crashing
- [x] 1.4 Rename `tui.toml` → `users.toml`: update `OVERRIDE_FILE_PATH`
  constant in `src/runtime/tui_session.rs`; update all test fixtures and
  inline TOML strings
- [x] 1.5 `users.toml` IDs: validate `session@GLOBAL` form on load; reject
  bare IDs without the suffix
- [x] 1.6 `TuiSession` struct: rename `policy_id` field to `policy` to match
  the TOML key already in the spec

## 2. Relay hello + identity (relay lane — `src/relay.rs`, `src/relay/stream.rs`)

- [x] 2.1 `StreamClientFrame::Hello`: remove `client_class` field
- [x] 2.2 `StreamServerFrame::HelloAck`: remove `client_class` echo; remove
  post-ack mismatch verification (relay.rs:931–935)
- [x] 2.3 `RelayStreamSession::new()`: remove `client_class` parameter and
  storage field (relay.rs:265, 762)
- [x] 2.4 `handle_hello_frame`: unified identity lookup — try bundle
  `[[sessions]]` first; if `session_id` carries `@GLOBAL` suffix, search
  `users.toml` instead; hydrate canonical `session_id@bundle_name` at
  registration
- [x] 2.5 All relay response and event fields that carry session identity
  (`target_session`, `sender_session`, `session_id` in `ListedSession`,
  `decided_by`): emit canonical `session@bundle` form

## 3. Relay routing and delivery (relay lane — `src/relay/**`)

- [x] 3.1 Delete `RelayClientClass` enum (`src/relay.rs`)
- [x] 3.2 Delete `RelayStreamClientClass` enum (relay.rs:233)
- [x] 3.3 Replace endpoint-class routing with session-type routing from config:
  `tmux` → prompt-injection/quiescence path; `acp` → ACP worker path;
  `ui` → stream push path; `pubsub` → fail-fast NYI

## 4. Permission decisioning (relay lane — `src/relay/authorization.rs`, `src/relay/handlers.rs`)

- [x] 4.1 Remove `UI-Mediated Decision Submitter Gate`: accept
  `permission.resolve` from any principal with `authorize_grant` capability
- [x] 4.2 Remove `operator-class` policy control from config validation and
  authorization evaluation
- [x] 4.3 Delete error codes `validation_invalid_client_class_for_action` and
  `validation_invalid_client_class_for_hello` from all error-emitting paths

## 5. MCP and TUI call sites

- [ ] 5.1 `src/mcp/mod.rs`: remove `client_class` from hello send; delete
  `is_session_operator_class_authorized`; update `src/mcp/README.md`
  "Permission Granting" section (remove `{ui, operator}` precondition prose)
- [ ] 5.2 `src/tui/state/mod.rs`: remove `client_class` from hello send;
  update canonical identity display where shown

## 6. Test cleanup

- [x] 6.1 Migrate unit/integration TOML fixtures to the revised schema
  (per-file `write_*` helpers updated in place; coder-backed sessions use a
  flat `coder` field, coder-less use `[sessions.ui]`)
- [x] 6.2 `tests/unit/relay_stream_client.rs`: remove `client_class` from
  hello construction and assertions
- [x] 6.3 `tests/integration/relay_delivery_runtime.rs`: update fixture
  configuration and any `client_class` assertions
- [x] 6.4 Update remaining integration tests: `tui.toml` → `users.toml`,
  flat `[[sessions]]` → revised schema; canonical `session@bundle` assertions
- [x] 6.5 Integration coverage refreshed: session-type routing; grant-only
  permission auth (`relay_permission_*`); `session@bundle` canonical wire
  form. (Session/coder mismatch rejection dropped — no longer possible.)

## 7. Data examples and docs (coordinator)

- [x] 7.1 Rename `data/configuration/tui.toml` → `data/configuration/users.toml`;
  add `[sessions.ui]` subtable to each entry; use `@GLOBAL`-form IDs
- [x] 7.2 Update `data/configuration/bundles/` examples: flat `[[sessions]]`
  entries → session-type subtable schema
- [ ] 7.3 Archive this change (`establish-session-type-taxonomy`) after all
  tasks complete
