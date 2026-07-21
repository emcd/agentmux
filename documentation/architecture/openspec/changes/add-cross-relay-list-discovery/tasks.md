## 1. MCP command schemas

- [x] 1.1 Replace `ListParams.command` with a serde/schemars enum containing
      `principals`, `namespaces`, `relays`, and `decisions`.
- [x] 1.2 Replace `UpdownParams.command`, `NewParams.command`, and
      `ChangeParams.command` with serde/schemars enums for their supported
      values.
- [x] 1.3 Preserve existing validation behavior for missing/unknown commands and
      unknown fields.
- [x] 1.4 Test that all four command schemas render as flat string enums
      (`type="string"` plus `enum`), not tagged or `oneOf` schemas.

## 2. MCP discovery surface

- [x] 2.1 Extend `ListArgs` with optional `relay`; preserve local namespace
      semantics when it is absent and require one concrete namespace when set.
- [x] 2.2 Add command-scoped argument types for `list.relays` and
      `list.namespaces`, with optional `relay` only on namespace discovery.
- [x] 2.3 Add `help` catalog/query support and exact schemas for `list.relays`,
      `list.namespaces`, and the updated `list.principals`.
- [x] 2.4 Implement handler branches for relay and namespace discovery and the
      foreign principal path.
- [x] 2.5 Emit `mcp.tool.list.relays.*`, `mcp.tool.list.namespaces.*`, and
      `mcp.tool.list.principals.*` request/success/relay_error/
      unexpected_response/io_error inscriptions as applicable (five lifecycle
      channels per command).
- [x] 2.6 Preserve `list.decisions` and local `list.principals` behavior.

## 3. Relay discovery contracts and dispatch

- [x] 3.1 Add relay request/response contracts for configured relay aliases,
      namespaces, and forwarded namespace/principal discovery.
- [x] 3.2 Ensure forwarded discovery requests carry neither an origin-local
      relay alias nor `on_behalf_of`, preventing transitive forwarding and
      attribution ambiguity.
- [x] 3.3 Extend `ListedBundle` with optional
      `principals_partial: Option<bool>`; use `Some(true)` only for filtered
      subsets and `None` for complete listings.
- [x] 3.4 Add relay-wide connection-layer and `src/relay/mod.rs` dispatch for
      configured relay listing and cross-relay discovery.
- [x] 3.5 Implement local relay alias enumeration without dialing peers or
      exposing address, `connect-as`, or credential details; iterate normalized
      `RelayRuntimeConfiguration.peers` aliases directly rather than querying
      `PeerConnectionManager`.
- [x] 3.6 Implement local namespace enumeration from the bundle catalog and
      `GLOBAL` view under the requester's `list` authorization.
- [x] 3.7 Reuse the canonical listed-bundle builder in
      `src/relay/handlers/listing.rs` for local and filtered foreign principal
      responses; do not duplicate readiness/state folding in discovery.
- [x] 3.8 Reject list scopes narrower than `all` in the new origin dispatch
      entries before any peer lookup or dial.
- [x] 3.9 Extend `PeerConnectionManager` to forward namespace/principal
      discovery using existing lazy dial and error classification.
- [x] 3.10 Implement receiving-relay discovery over only its local catalog and
      registry, reusing `RouteAuthorization::Ingress`/`scope_permits` semantics.
- [x] 3.11 Emit `relay.discovery.*` request/success/relay_error/
      unexpected_response/io_error inscriptions across discovery dispatch.

## 4. Authorization and validation tests

- [x] 4.1 Schema: `list.command` advertises all four enum values and command help
      marks `relay`/`namespace` required or optional as specified.
- [x] 4.2 Local: `list.relays` returns sorted aliases without dialing and returns
      an empty array when no peers are configured.
- [x] 4.3 Local: `list.namespaces` reflects only namespaces permitted by the
      requester's `list` scope.
- [x] 4.4 Foreign: origin requester needs `list` scope `all` before the peer is
      contacted.
- [x] 4.5 Foreign: absent peer ingress scope returns `authorization_forbidden`.
- [x] 4.6 Foreign: namespace scope exposes complete principals and exact-principal
      scope exposes a subset with `principals_partial=true`.
- [x] 4.7 Foreign: a namespace-scoped grant for an empty namespace omits that
      namespace and returns an empty namespace list without existence disclosure.
- [x] 4.8 Foreign: an out-of-scope concrete namespace returns
      `authorization_forbidden` without namespace-existence disclosure.
- [x] 4.9 Foreign: unknown alias, no configured peers, missing credential, and
      unreachable/authentication failure preserve their typed errors.
- [x] 4.10 Foreign: peer validation/authorization errors propagate unchanged and
      foreign bundle ids are not rewritten or synthesized.
- [x] 4.11 Foreign: returned namespace ids derive from the receiving relay's
      local catalog/registry and cannot be injected by the origin request.
- [x] 4.12 Wire: forwarded discovery contains neither the origin `relay` selector
      nor `on_behalf_of` and cannot trigger an onward peer lookup.
- [x] 4.13 Validation: foreign principal listing rejects omitted, empty, `*`, and
      unsupported reserved namespaces; foreign relay selectors reject empty and
      `*` for both namespace and principal discovery.

## 5. Documentation and validation

- [ ] 5.1 Update `src/mcp/README.md`, `src/mcp/server/handlers/README.md`, and
      `src/relay/README.md` for the discovery commands and trust boundaries.
- [ ] 5.2 Run `openspec validate add-cross-relay-list-discovery --strict`.
- [ ] 5.3 Run `cargo fmt --check`.
- [ ] 5.4 Run `cargo clippy -- -D warnings`.
- [ ] 5.5 Run `cargo nextest run --locked --config-file .auxiliary/configuration/nextest.toml`.
