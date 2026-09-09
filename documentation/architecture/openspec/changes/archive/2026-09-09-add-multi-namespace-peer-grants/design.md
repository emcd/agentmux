## Context

The operator approved breaking pre-1 peer grant semantics, `*` covering all
addressable namespace types immediately, comma-separated namespace sets, and
dynamic updates preserving credentials and unrelated record state. This
revision supersedes the earlier draft's policy recommendations. The assignment
remains proposal-only. Baseline inspection is local master `9a71fdf3`;
`agentmux:issues/relay/82` is the single notebook tracking anchor.

Relevant existing implementation:

| Seam | Evidence and consequence |
|------|--------------------------|
| Principal store | `src/relay/identity.rs` stores `scope: Option<String>` and persists by atomic sibling-file rename, the last fallible store step. No new persisted field or compatibility layer is needed. |
| Rights | `VerifiedIdentity`, `connection/hello.rs`, `connection/serve.rs`, and `context.rs` currently copy peer scope at Hello. Those copies cannot authorize live-updated grants. |
| Shared scope helper | Application introspection, snapshots, and trusted-host revocation filtering also use the existing application scope helper; peer parsing must be separated rather than broadening application privileges. |
| Admin serialization | `ConnectionServeContext.identity_admin_lock` serializes load/stage/persist transactions. `handlers/dispatch.rs` routes relay-wide admin before bundle dispatch. New scope updates must use both seams. |
| Policy | `authorization/loading.rs` parses action maps under new/change/drop with no fixed action roster; `authorize_relay_action` requires the named action at `all`, missing means denied. |
| Starter policy | `data/configuration/policies.toml` explicitly grants operator `new.peer`, `change.psk`, and `drop.peer`; add `change.scope` explicitly, not by inference from rotation rights. |
| Delivery | `authorization/checks.rs` chooses policy or ingress authorization; preparation validates existence first. The grant/admission interval needs ordering with scope commits, not merely a fresh read at dispatch. |
| Admission seam | `handlers/routed.rs` currently authorizes before its execute closure; `handlers/send.rs` and `handlers/raww.rs` import the synchronous `delivery::admission::admit` used later by execution. The shared spine must retain authority through admission without putting policy parsing into operation bodies. |
| Discovery | `handlers/discovery.rs` independently tests target coverage, namespace coverage, and exact-principal scope, using catalog and GLOBAL registry data. Replace these with current namespace grant filtering; no new lookup shapes. |
| Adapters | `commands/new.rs`, `commands/change.rs`, `mcp/params.rs`, handlers and generated help carry string scopes and a psk-only change selector today. |

## Goals / Non-Goals

**Goals:**

- One peer identity/credential/endpoint reaches a set of receiving namespaces.
- Wildcard grants cover GLOBAL, future bundles, and future addressable types
  without reissuance, reconnect, or a second namespace allowlist.
- Administrators can replace or clear a peer grant in place without changing
  the PSK, identity, expiry, unrelated metadata, or connection.
- Update success is a reliable ordering boundary for subsequent authorization.

**Non-Goals:**

- Discovery aggregation, namespace recipient fanout, broadcast expansion,
  multi-hop, new target syntax, or new operations such as cross-relay Look.
- Application introspection expansion, per-origin/per-operation peer ACLs,
  credential distribution, expiry-timer repair, or queued-delivery cancellation.
- Legacy peer grant preservation, migration adapters, dual persisted versions,
  optimistic-concurrency API fields, or a new top-level MCP tool.

## Decisions

### 1. A single string grammar for peer scope

The persisted `scope` and CLI/MCP/relay scope inputs use one grammar:

| Input | Meaning |
|-------|---------|
| `*` | Every namespace with addressable principals. |
| `alpha,beta,GLOBAL` | All principals in each named namespace. |
| `alpha` | A one-element namespace set. |
| Empty/whitespace-only string | No ingress rights; clear on update. |
| Omitted/null at registration | No ingress rights. |

Trim surrounding whitespace on the whole string and each comma-separated item;
sort and deduplicate explicit names, preserving case. Persist the canonical
comma string, `*`, or absent scope for no rights. `change scope` requires a
string, including the explicit empty string to clear; omitted/null is not an
update. Empty list elements, wildcard mixed with names, malformed names, and
unsupported reserved non-addressable partitions are ordinary validation errors.
Valid namespace names need not exist yet. Use the runtime's concrete namespace
grammar and addressability classification, not a second hard-coded list of
allowed namespace types. `RELAY` and `EXTERNAL` are not addressable today.

The wildcard is not an enumeration of current bundles. It automatically covers
new namespace types as soon as they become addressable, with no extra grant
switch or GLOBAL exception. It authorizes targets; it does not make an
unsupported target addressable or implement a new operation. Future namespace
types use whatever candidate enumeration their own discovery implementation
provides; this change adds no new catalog or discovery aggregation mechanism.

The word `all` is not wildcard syntax; under the new grammar it is an ordinary
namespace name. Existing policy-tier vocabulary advisories can remain for
single-name `none`, `self`, `home`, and `all`, explaining `*` when wildcard
reach was intended. This is current syntax, not a legacy parsing branch.

Exact-principal peer grants are removed from the contract. All stored peer
scopes use the new parser, including at startup and authoritative read;
invalid peer records fail loading rather than acquiring default or implicit rights. No
special migration, version upgrade, or backwards-compatible reader is added.
The application scope model stays separate and unchanged.

### 2. Minimal dedicated scope administration

```sh
agentmux new peer west@RELAY --scope 'alpha,beta'
agentmux new peer east@RELAY --scope '*'
agentmux change scope west@RELAY --scope '*'
agentmux change scope west@RELAY --scope ''
```

MCP uses existing `new`/`change` tools:

```json
{"command":"scope","args":{"principal_id":"west@RELAY","scope":"alpha,GLOBAL"}}
```

Add relay `ChangeScope { principal_id, scope: String }`, dispatched relay-wide
before bundle routing. Require a valid `@RELAY` target and an explicit scope
string. A missing target record returns `validation_unknown_principal` after
authorization. Shape/type/grammar failures use `validation_invalid_params`
with field information. No credential destination applies. Success returns
`schema_version`, `principal_id`, and canonical `scope` (empty string for no
rights), with optional existing-style vocabulary diagnostics. Repeating the
same normalized scope succeeds without rotation or disconnect.

Use the existing named-action mechanism:

```toml
[policies.controls.change]
psk = 'all'
scope = 'all'
```

Only `change.scope=all` authorizes scope replacement, whether widening,
narrowing, clearing, or no-op. `none`, `self`, `home`, and missing deny.
`new.peer`, `change.psk`, and `drop.peer` confer no scope-update permission;
scope-update rights confer no credential-rotation or creation rights.
The grant on a peer never grants administrative authority. Add the explicit
scope control to the starter operator preset; do not rewrite existing operator
configuration or infer the new right from existing controls.

Alternatives rejected: reusing rotation permission conflates credentials with
authorization; updating scope through `change psk` couples it to a new secret;
drop/re-register violates the preservation and uninterrupted-connection goals.

### 3. Current grant and a shared linearization boundary

Hello establishes authenticated peer identity, not durable authorization rights.
For each ingress Send/Raww or discovery operation, resolve the current record
for that authenticated peer from the authoritative store. A missing record or
invalid authority cannot authorize. Do not accept a request-supplied scope or
foreign `on_behalf_of` as authority. Preserve the connection's credential
binding so a dropped/recreated identity cannot revive an obsolete connection
during revocation races merely because its principal id was reused.

Recommend extending the existing relay-scoped identity-admin serialization to
cover the final ingress authorization/admission transaction. Read the record
under that boundary; do not introduce an asynchronously refreshed grant cache.
All registration, rotation, drop and scope-update transactions share the same
ordering domain so concurrent record writes cannot restore a stale grant or
lose another field's update. Loading and persistence remain blocking-pool work.

For scope updates, construct an updated copy of the record, changing scope
only. Persist through the existing atomic-store commit: sync the temporary
file, rename it over the store file, then sync the parent directory before
acknowledging success. The successful rename is the update linearization
point for effective authority while the shared boundary is held; publish
no new runtime rights before that rename, and keep the published replacement
authoritative while directory synchronization completes. Do not unlock with
disk replacement published but the old in-memory grant still authoritative.
Release before sending the response. A validation, authorization, temporary
file, or pre-rename persist failure leaves both durable and effective old
state intact. A post-rename directory-sync failure leaves publication already
effective: the replacement scope governs current authorization, the caller
receives an indeterminate-durability error rather than success or an
old-grant-intact claim, and the response reports the effective replacement
scope when scope is exposed. Do not
prune expiry or alter unrelated records as a side effect of this operation.
Response transport loss after commit is not an update failure: the outcome is
unknown to the caller, and repeating the replacement safely establishes it.
A same-scope retry SHALL NOT short-circuit as a no-op; it SHALL reauthorize
under current credentials and controls, perform the directory synchronization,
and follow ordinary last-writer-wins explicit-request semantics without
overwriting an intervening committed update through any retry exemption.

For delivery, the authorization linearization point is the final grant check
and admission transaction. An earlier read/prepare is not permission to admit
later. Prevent a scope update from interleaving between this final check and
admission; alternatively revalidate under that same boundary immediately at
admission. The simple recommended ordering holds the boundary through local
admission, never through asynchronous delivery execution. Define lock ordering
with the admission ledger and never await or wait for a target under this lock.
Preserve existing all-target authorization: if any resolved target is outside
scope, none is admitted. This does not add new atomicity for unrelated delivery
failures or new recipient expansion.

Admission here is relay acceptance into the delivery queue through the
admission ledger, not a transport's later mailbox declaration, submission or
terminal outcome. The grant is not checked again to cancel work at those later
execution stages. Refactor the shared spine/admission handoff as necessary to
carry the ordering guard; do not move policy decisions into operation bodies.

For discovery, the authorization linearization point is taking the current
grant and fixing the scope-filtered result under the same ordering boundary.
Candidate collection can occur earlier; final filtering and the decision must
use the current record. Socket response writing occurs afterwards. This is a
grant-consistent operation, not a transactional snapshot of bundle lifecycle.

| Race ordering | Required result |
|---------------|-----------------|
| Delivery admission precedes scope commit | It may execute after update success; no retroactive cancellation. |
| Scope commit precedes final delivery authorization/admission | Only the replacement scope may authorize; queued-but-not-admitted preparation gains no grandfathered access. |
| Discovery decision precedes scope commit | Its already-fixed result may arrive after update success. |
| Scope commit precedes discovery decision | Both namespace and principal discovery use only the replacement scope. |
| Failed scope transaction before rename publication | Subsequent authorization uses the old scope. |
| Rename published but directory sync failed | The replacement remains effective; the caller receives indeterminate-durability uncertainty, not success and not an old-grant-intact claim. |
| Concurrent successful scope updates | They serialize; the last committed replacement is authoritative, not a merged union. |

Thus even a request started before the update must use the new scope if it
has not crossed its authorization/admission point. After a successful update
response, every subsequently ordered authorization sees the new or a still
later committed grant. Existing peer connections remain connected. No
credential revocation event is emitted for a scope-only update; record the
actor, peer and old/new canonical scope in local inscriptions, never the PSK.

Alternatives rejected: Hello snapshots retain removed access; per-request reads
without admission ordering allow TOCTOU admission; per-operation generation
caches add invalidation state when the existing transaction seam can suffice.

### 4. Consequences for existing discovery only

Reuse the namespace coverage predicate for Send/Raww and discovery. The origin
still requires the relevant local control at `all`, and receiving peer scope
remains operation-agnostic. Target existence-before-authorization ordering
remains unchanged; do not claim new Send non-disclosure guarantees.

Namespace discovery filters its existing local candidates to covered namespaces
containing at least one addressable principal. Results remain sorted and unique.
Wildcard includes GLOBAL whenever it has such a principal. Empty/absent scope
denies discovery. A nonempty set covering no candidate returns empty success
with the existing local scope-unmatched inscription. Out-of-scope principal
lookup denies without revealing namespace existence; a covered absent
namespace returns a neutral empty view, as before.

Foreign principal discovery still names exactly one concrete namespace. Because
peer grants now cover whole namespaces, it returns a complete covered namespace
and its normal diagnostics, with no scope-induced `principals_partial` marker.
The generic partial marker is not removed from other contracts. The previous
exact-principal peer subset and diagnostic-suppression path is no longer part
of this model. No GLOBAL view is appended to a different foreign namespace
lookup. Existing candidate sources, aliases, forwarding and outcome propagation
stay intact; no `namespace='*'` foreign principal aggregation is introduced.

### 5. Affected-spec audit

- `relay-identity`: add peer-only grammar and live-authority/atomic-update
  requirements. Do not modify application introspection, its exact scopes,
  credential-expiry timing, or historical unrelated identity prose.
- `relay-routing-layer`: fully replace target ingress and discovery filtering
  requirements, keeping origin/receiver boundaries, existence ordering,
  advisory attribution, empty namespace non-disclosure and unmatched logging.
  Intentionally drop `Exact principal scope exposes partial namespace`, since
  exact-principal peer grants no longer exist. No other live scenario is dropped.
- `authorization-scope`: add dedicated scope-admin action using the existing
  optional action-map pattern, not a new policy ladder or mandatory config key.
- `mcp-tool-surface`: modify New, Change and Change Success requirements;
  retain all PSK-specific obligations and scenarios. Add scope-update contract
  and help discovery alongside existing commands. Keep vocabulary advisories
  and credential destinations; they are independent of compatibility.
- `cli-surface`: generic help topology already covers the new subcommand;
  grant-admin syntax lives with the corresponding MCP contract. No delta.
- `cross-relay-routing`, `addressing-routing`, `runtime-bootstrap`: no new
  routing, listing shape, peer endpoint, or bootstrap configuration schema.
  References to registered peer scope now refer to the new semantics. No delta.

## Risks / Trade-offs

- Wildcard grants expand with future addressability -> this is the explicitly
  chosen policy; document it prominently and offer sets for bounded reach.
- Serialization adds contention around disk reads and admission -> keep the
  critical section bounded to authority and local admission/final filtering,
  not target execution or network response writing; test deadlock-free ordering.
- Updated grammar breaks existing peer records -> deliberate pre-1 cutover;
  document operator reprovisioning of invalid state, not conversion machinery.
- A discovery response may arrive after a narrowing it preceded -> document
  decision ordering, not an impossible promise to retract bytes already decided.
- Lost update response after durable commit, or an indeterminate-durability
  error after rename publication, -> idempotent replacement can be retried
  with reauthorization and directory synchronization; do not roll back a
  published grant because the client disconnected or durability is uncertain.

## Migration Plan

1. Implement only after a separate implementation dispatch. Upgrade the relay
   and local CLI/MCP together; no mixed-version admin compatibility is promised.
2. Stop the relay before deploying the breaking grammar. Review/reprovision
   peer scope state using the new namespace grammar; invalid old peer state
   is not automatically repaired or converted. No store-version migration is
   required because the persisted field remains a string.
3. Existing configurations must explicitly grant `change.scope='all'` to
   intended administrators. Starter hydration still never overwrites them.
4. Use `change scope` for ordinary live grant updates. Verify on the same peer
   connection that narrowing affects Send/Raww and discovery while its PSK,
   identity, expiry and unrelated metadata remain unchanged.
5. Binary rollback is an operator-managed state review, not a supported grant
   conversion path. Do not run older binaries against new scope semantics and
   assume equivalent authority. Restoring a backup requires reviewing subsequent
   revocations/rotations; it must not silently resurrect access.

## Open Questions

No material policy ambiguity remains. The namespace grammar, wildcard reach,
live update preservation and ordering are operator decisions. `change scope`,
`change.scope=all`, explicit empty-string clearing, canonicalization and the
existing transaction-lock approach are concrete minimal design recommendations,
not prerequisites for another policy round. Implementation must verify the
actual lock ordering and final-admission seam with deterministic race tests.
