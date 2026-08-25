# Authorization

This reference describes the authorization model: per-control policy
scopes, the home-namespace concept, and the reachability rules
operators and agents rely on. The README keeps a short pointer; this
doc carries the full model.

## Policy presets and controls

Authorization for relay operations (`list`, `look`, `send`, `raww`,
`choose`, `updown`) resolves against named policy presets declared in
`<config-root>/policies.toml`. Each preset maps each control to one
of `none`, `self`, `home`, or `all`. The configured scope has to meet
the operation's minimum.

The starter template ships two presets:

- `default` — conservative same-bundle policy; `find`, `list`,
  `look`, and `send` are explicitly set, while `raww`, `choose`, and
  `updown` are intentionally omitted (they resolve to their built-in
  `none` default; an omitted control is not an override).
- `operator` — cross-namespace inspection, messaging, choice
  decisions, and lifecycle. Every control is explicitly set,
  including `raww`/`choose`/`updown`.

For the file schema and starter templates, see
[maintainer-configuration-guide.md](maintainer-configuration-guide.md).

## The per-control ladder

Per-control scopes form a ladder:

- `self` — act only on yourself.
- `home` — act on any principal in your own/home namespace.
- `all` — act across namespaces.

A scope at a higher tier is strictly more permissive than a scope at
a lower tier; a `home` grant implies `self`, an `all` grant implies
both.

## Home namespace

A principal's *home* is its native namespace:

- A session's home is its bundle.
- A relay-wide principal (such as a `@GLOBAL` operator) lives in its
  reserved namespace: `GLOBAL`, `EXTERNAL`, or `RELAY`.

Reaching *into* a bundle you do not live in requires `all`, so a
`@GLOBAL` operator needs `all` to list or message a bundle's
sessions.

Messaging the operator is the practical exception that holds for
`send` only. A bundle agent whose own policy grants `send = "home"`
can message an `@GLOBAL` operator on `send`; the relay's authorization
routing classifies `@GLOBAL` at the sender's home tier for `send`,
so a sender in any bundle reaches the operator without an explicit
`all` grant.

The capability gates reject `@GLOBAL` UI principals as targets for
`look` and `raww` — those handlers treat a relay-wide target as
delivered-via-stream rather than coder-session, so the registry
returns `unsupported_operation`
(`src/relay/handlers/look.rs:155-170`,
`src/relay/handlers/raww.rs:296-316`). A bundle agent replying to
the operator therefore uses `send`, not `look` or `raww`.

The starter `operator` policy (`data/configuration/policies.toml:28-43`)
grants `send = "all"` rather than `home`: the operator itself is
`@GLOBAL`, whose home is `GLOBAL` and not any specific bundle, so
`home` would confine the operator to messaging only the relay-wide
`@GLOBAL` view. The `all` grant lets the operator message into
bundles instead.

## `updown` is deny by default

The `updown` control gates the `agentmux up` and `agentmux down`
commands (and the corresponding `RelayRequest::Up` /
`RelayRequest::Down` requests to the relay). It is **deny by
default** — a session whose policy does not grant `updown = "home"`
cannot bring bundles up or down. Configured operators (the starter
`operator` policy in the scaffolded `policies.toml`) carry this
grant; the conservative `default` policy does not.

A bundle lifecycle request without an authorized principal receives a
typed `authorization_forbidden` validation error from the relay.
`map_relay_error` (`src/commands/shared.rs:176-192`) routes that
code through `RuntimeError::validation` rather than the IO-status
path that wraps internal relay codes as `relay error <code>`, so
the CLI surfaces a typed validation result with the relay's
operator-facing message rather than a `relay returned error: ...`
wrapper. Operators who hit this should verify that `users.toml` maps
their `session@GLOBAL` identity to a policy with `updown = "home"`.

## Relay-wide credential administration

The `new`, `change`, and `drop` tools are relay-wide operations: they
mutate the relay-level principal store, so a namespace-relative `home`
grant is insufficient. A bundle-relative operator who tries to mint a
peer PSK from within their bundle receives `authorization_forbidden`.

The relay-wide grants are:

- `new.peer = "all"` — register a peer principal and mint its PSK.
- `change.psk = "all"` — rotate an existing principal's PSK.
- `drop.peer = "all"` — delete a principal from the store, revoking any
  session still bound to it.

These are three separate controls. Granting `new.peer` and `change.psk`
confers no ability to drop, so an existing policy file that predates the
`drop` control permits no deletion until an operator adds it.

These grants must be carried by the calling session's policy preset,
not by the bundle's `default` preset. The MCP server's own identity
must therefore carry an operator policy for these tools to succeed.

## Operational references

- See [maintainer-configuration-guide.md](maintainer-configuration-guide.md)
  for file-by-file configuration root contents, layering, and starter
  hydration.
- Shared runtime flags and relay-host defaults:
  [operations.md](operations.md).
- Multi-worktree topology and association resolution:
  [multi-worktree-workflow.md](multi-worktree-workflow.md).
- MCP tool inventory and delivery behavior:
  [mcp-surface.md](mcp-surface.md).
