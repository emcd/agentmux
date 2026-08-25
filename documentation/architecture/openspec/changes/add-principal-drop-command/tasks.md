## 1. Authorization Family

- [x] 1.1 Add `RelayActionFamily::Drop` with the `drop` namespace in
  `src/relay/authorization/checks.rs`.
- [x] 1.2 Add the `drop_controls` map to the resolved relay principal controls
  and its loader parsing, alongside `new_controls` and `change_controls`.
- [x] 1.3 Test that an `all`-scoped `drop.peer` grant authorizes and that
  `home`, absent, and `new.peer`/`change.psk`-only grants each forbid.

## 2. Relay Drop Handler

- [x] 2.1 Add `RelayRequest::DropPeer` and `RelayResponse::DropPeer` with the
  payload fields the spec requires.
- [x] 2.2 Implement `handle_drop_peer` in `src/relay/handlers/identity.rs` in
  this order: reject self-drop, authorize, load the store, reject an
  unregistered id with `validation_unknown_principal`, delete the record,
  persist, then revoke.
- [x] 2.3 Reject dropping the requester's own principal with
  `validation_self_drop_forbidden` before authorization and before any store
  access, so an ungranted caller dropping their own id gets the validation
  error rather than an authorization denial.
- [x] 2.4 Reuse `revoke_streams_for_identity` and
  `notify_trusted_hosts_of_revocation` so dropping emits the same
  `runtime_identity_revoked` frame and `identity.revoked` event as rotation.
- [x] 2.5 Report `credential_path` for session principals only, omitting it for
  relay, user, and application principals, and deleting no file.
- [ ] 2.6 Test that a failed store persist revokes nothing. Not covered: the
  ordering is enforced structurally, since `persist()?` returns before the
  revocation block, and no cheap seam induces a persist failure after a
  successful load — `persist` resets its own parent directory to `0o700`, so the
  read-only-directory technique the sibling tests use cannot reach it.
- [x] 2.7 Test that self-drop mutates nothing and revokes nothing, that an
  ungranted self-drop returns `validation_self_drop_forbidden` rather than
  `authorization_forbidden`, that an ungranted drop of an unregistered
  principal returns `authorization_forbidden` rather than disclosing the
  principal's absence, and that dropping a peer relay principal omits
  `credential_path` rather than reporting a path under the dropping relay's
  state root.

## 3. CLI Surface

- [x] 3.1 Add `src/commands/drop.rs` implementing `agentmux drop peer
  <principal_id>` with the shared runtime, `--bundle`, `--as-session`, and
  `--json` flags.
- [x] 3.2 Wire `drop` into command dispatch and the top-level help listing.
- [x] 3.3 Test the human and `--json` output shapes, and that an unregistered
  principal exits non-zero.

## 4. MCP Surface

- [x] 4.1 Add the `drop` meta-tool handler and `DropPeerArgs` validation
  mirroring the `new`/`change` handlers.
- [x] 4.2 Advertise `drop` in tool enumeration.
- [x] 4.3 Treat `drop` as a relay-backed tool, so a call on an unassociated
  server is rejected with `validation_unassociated_server` before relay contact.
- [x] 4.4 Test tool advertisement, a successful drop, the unknown-principal
  rejection, and the unassociated-server rejection.

## 5. Mint-Time Scope Diagnostic

- [x] 5.1 Add an optional `diagnostics` array of `code`/`message` entries to
  `RelayResponse::NewPeer`, omitted when empty.
- [x] 5.2 Attach an `advisory_scope_resembles_policy_tier` diagnostic when
  `new peer` receives a `scope` of `none`, `self`, `home`, or `all`, without
  failing the request.
- [x] 5.3 Preserve diagnostics in the MCP `new` structured result.
- [x] 5.4 Render diagnostics to stderr in the CLI `new peer` path, including the
  `--json` mode, and keep the exit status zero.
- [x] 5.5 Test that each of the four values warns and still registers, that both
  surfaces receive the diagnostic, that the command exits zero, and that an
  unrelated unresolvable scope produces no diagnostics field at all.

## 6. Validation

- [x] 6.1 Teeth-check the revocation reuse: suppress the revoke call and confirm
  the live-session test fails.
- [x] 6.2 Run fmt, clippy, `openspec validate --all --strict`,
  `scripts/verify-openspec-deltas.py`, and the full nextest suite.
