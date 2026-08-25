## Why

The principal lifecycle has no terminal operation. `new peer` mints a principal
and `change psk` rotates its credential, but nothing drops one. A principal
minted with a wrong field — most commonly an ingress `scope` that can never
match a target — is unrecoverable through any designated surface: `new peer`
refuses an id that already exists, `change psk` preserves scope by design, and
no drop command exists in the CLI or the MCP tool surface. The only
correction available is hand-editing the persisted principal store, which
bypasses every management surface the relay owns.

This has been exercised live twice: a mis-scoped peer credential that had to be
corrected by editing the store directly to unblock cross-relay testing, and an
orphaned peer record that has remained undeletable since it was minted.

The revocation contract this needs already exists. `relay-identity`'s
*Revocation and Expiry Enforcement* requirement specifies what the relay does
when a principal is explicitly revoked: tear down every bound session, emit a
`runtime_identity_revoked` typed error frame before closing, and emit an
`identity.revoked` event to trusted-host streams in scope. Today that machinery
has exactly one caller — PSK rotation — where the principal continues to exist.
The revocation path is fully specified and unreachable by any operator action.

## What Changes

- Add a `drop` meta-tool and a corresponding `agentmux drop peer
  <principal_id>` CLI command that deletes a principal record from the
  relay-wide principal store.
- Dropping a principal invokes the existing revocation contract: bound sessions
  are torn down with a `runtime_identity_revoked` frame, and trusted-host
  streams in scope receive `identity.revoked`.
- `drop` is a relay-wide operation, authorized against an `all`-scoped
  `drop.peer` grant. A bundle-relative `home` grant is insufficient, matching
  `new.peer` and `change.psk`.
- Dropping the requester's own principal is rejected outright. Revocation
  would tear down the connection carrying the request, losing the response to an
  operation that had already committed.
- Dropping a principal does not delete credential files. The response reports
  the canonical credential path for session principals only; for a peer relay
  the credential lives under the connecting relay's state root, which the
  dropping relay cannot observe.
- `new peer` warns, without failing, when `--scope` is given a value from the
  policy-tier vocabulary (`none`, `self`, `home`, `all`). These are not
  ingress-scope values, and `all` in particular reads as a wildcard while
  matching nothing.
- Successful `new` responses gain an optional `diagnostics` array carrying a
  `code` and `message`. The relay is a separate process whose stderr reaches
  neither the CLI nor an MCP client, so an advisory has to travel in the payload
  to reach either caller; the CLI renders it to its own stderr.

## Capabilities

### New Capabilities

None. This extends existing identity-administration surfaces.

### Modified Capabilities

- `mcp-tool-surface`: adds the `drop` meta-tool, its request `args` and
  success payload contracts, and its authorization grant; adds the policy-tier
  collision warning to the `new` tool's `scope` handling, and the `diagnostics`
  field that carries it to the `new` success payload contract; replaces the
  Error Object Contract's flat validation-before-authorization rule with the
  privileged-state boundary that dropping's non-disclosure behavior depends on,
  and adds `drop` to its relay-backed tool list.

## Impact

- `src/commands/`: new `drop.rs` command module; help topology; `new.rs`
  scope warning.
- `src/mcp/`: new `drop` tool handler and argument validation.
- `src/relay/`: new `DropPeer` request/response variants; `handle_drop_peer`
  in `handlers/identity.rs` reusing the existing revocation helpers;
  `RelayActionFamily::Drop` and its `drop_controls` policy map.
- Policy configuration gains a `drop` control namespace. Deployments without
  a `drop.peer` grant cannot drop principals, which fails closed.
- No change to the principal store's on-disk format; `remove_by_principal_id`
  already exists and is used by rollback paths.
