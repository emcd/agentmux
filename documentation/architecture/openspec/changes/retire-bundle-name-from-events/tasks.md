## 1. Relay — qualify event target_session and remove bundle_name (BE)

- [ ] 1.1 Audit every `RelayStreamEvent { ... }` construction site in
      `src/relay/delivery/` and `src/relay/handlers/` — confirm the
      TOP-LEVEL `target_session` is emitted in canonical `session@bundle`
      form; fix all bare-id sites. Known gaps:
      - `permission.snapshot` (permission_state.rs:281)
      - `permission.requested` (permission_state.rs:527) — top-level only;
        the PAYLOAD `target_session` is already canonical via
        `canonical_session_id()` and must not be changed
      - `permission.resolved` (permission_state.rs:494)
      - `incoming_message` top-level target (ui_delivery.rs:88)
      - target-facing `delivery_outcome` (ui_delivery.rs:211)
      - sender-facing `delivery_outcome` (async_worker.rs:453) — routes to
        sender's bundle but describes a target in the target's bundle; must
        qualify as `session@TARGET_BUNDLE`, not `session@SENDER_BUNDLE`
- [ ] 1.2 Remove `bundle_name` field from `RelayStreamEvent` in both
      `src/relay/contract.rs` (public wire type) and `src/relay/stream.rs`
      (pub(super) server-side emit type); remove the per-recipient
      bundle_name rewrite at stream.rs:493-495; fix all construction sites
- [ ] 1.3 Remove `bundle_name` from `RelayResponse::Send`,
      `RelayResponse::Look`, and `RelayResponse::PermissionList` in
      `src/relay/contract.rs`; fix all construction sites in handlers

## 2. Relay — tests (BE)

- [ ] 2.1 Update integration test fixtures in `tests/` that construct
      `RelayStreamEvent` with `bundle_name` or bare `target_session` values
- [ ] 2.2 Update assertions on Send/Look/PermissionList responses that check
      for `bundle_name` field

## 3. TUI — test fixtures (FE)

- [ ] 3.1 Update inline `RelayStreamEvent { bundle_name, target_session, ... }`
      fixtures in `src/tui/state/mod.rs` unit tests — remove `bundle_name`,
      qualify `target_session` to `session@bundle` form
- [ ] 3.2 Confirm `record_stream_events` tests still pass; no behavioral
      changes expected (TUI never reads `event.bundle_name`)

## 4. MCP — handler and test updates (AE)

- [ ] 4.1 Audit confirms `bundle_name` is explicitly re-emitted at three
      sites; removal is live work:
      - send handler (server/handlers/send.rs — was server.rs:240, :250, :265)
      - look handler (server/handlers/look.rs — was server.rs:339, :349, :406)
      - grant list handler (server/handlers/grant.rs — was server.rs:607, :613, :619)
- [ ] 4.2 Remove `bundle_name` destructure and re-emit from the three
      handler files identified in 4.1; remove from success inscriptions
- [ ] 4.3 Update integration test fixtures in `tests/integration/mcp/`
      — drop `bundle_name` from mocked relay responses and remove assertions
      on `payload["bundle_name"]` in look.rs and grant.rs

## 5. Specs — archive and reconcile

- [ ] 5.1 After implementation, run `openspec validate --strict` and resolve
      any issues
- [ ] 5.2 Archive `retire-bundle-name-from-events` with spec updates to
      `session-relay` and `mcp-tool-surface`
