## Why

A peer credential currently covers only one principal or literal namespace,
forcing duplicate identities and routes for several bundles on one relay.
For example, an operator hosting five bundles today needs five separate
`west-N@RELAY` peer identities and five routing configurations to reach them
all; adding a sixth bundle or GLOBAL access means provisioning another
credential and touching routes again. Namespace sets, an explicit wildcard,
and live grant replacement let operators manage one peer relationship without
reissuing credentials or retaining stale authorization
(`agentmux:issues/relay/82`).

## What Changes

- **BREAKING**: peer `scope` becomes `*` for all namespaces with addressable
  principals, or a comma-separated set of explicit namespaces. Exact-principal
  peer grants are outside the new grammar. No legacy grant parser, dual store
  format, migration adapter, or compatibility preservation is provided.
- `*` includes GLOBAL, future bundles, and newly introduced addressable
  namespace types immediately. It is not a snapshot or a bundle-only allowlist.
- Keep `new peer --scope` and MCP `new.peer` string input; add dedicated
  `change scope` / MCP `change.scope` administration with a distinct
  `change.scope=all` policy control. An empty scope clears the grant.
- Replace scope in place, preserving PSK, principal identity, expiry and
  unrelated metadata. Rotation remains separate and cannot change a grant.
- Every ingress operation, including existing discovery, consults the current
  authoritative grant. Scope update commit is ordered against final admission
  and discovery authorization. Authorization and pre-rename failures leave the
  old record intact; post-rename directory-sync failure keeps the published
  replacement effective and reports indeterminate durability. Previously
  admitted deliveries are not retroactively cancelled.
- Adapt existing discovery filtering to complete namespace grants and current
  authorization. Do not add discovery aggregation, namespace recipient fanout,
  onward forwarding, or new operations such as cross-relay Look.
- Leave application introspection and its separate grant model unchanged.

## Capabilities

### New Capabilities

None. This extends existing capabilities.

### Modified Capabilities

- `relay-identity`: Peer scope grammar, authoritative current grants, atomic
  in-place update, and ordering against admission and discovery.
- `relay-routing-layer`: Set/wildcard ingress coverage and consequences for
  existing receiving-side discovery filtering.
- `authorization-scope`: Dedicated, deny-by-default `change.scope` action.
- `mcp-tool-surface`: Provisioning syntax, scope-update command and payloads,
  CLI equivalents, generated help and separate rotation behavior.

## Impact

Implementation would touch the principal store, peer authentication binding,
relay-wide identity-admin serialization, delivery authorization/admission,
discovery handlers, relay contracts, CLI/MCP dispatch and schemas, starter
operator policy, and usage/subsystem documentation. No new dependency, tool
family, or per-namespace endpoint is needed.

The CLI topology already requires help to match dispatch without enumerating
commands; credential/grant administration is specified with its MCP equivalent.
No CLI topology delta or forwarding/addressing expansion is necessary.

The operator has explicitly approved the wildcard coverage, breaking grant
semantics, and dynamic-update security contract. This revision supersedes the
earlier compatibility-oriented recommendations. It remains proposal-only:
no implementation, merge, or push is authorized.
