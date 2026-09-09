## ADDED Requirements

### Requirement: Peer Ingress Scope Grammar

The relay SHALL interpret a relay principal's `scope` as `*` for every namespace
with addressable principals, or a comma-separated set of explicit concrete
namespace names. A single name SHALL be a one-element set. Empty or absent
scope SHALL confer no ingress rights. Namespace sets SHALL cover all
addressable principals in their named namespaces, not individual principals.

The wildcard SHALL include GLOBAL, future bundles, and newly introduced
addressable namespace types immediately, without reconnect, reissuance, a
catalog snapshot, or an extra inclusion switch. Scope SHALL NOT make a
non-addressable namespace or an unsupported operation addressable.

Parsing SHALL trim surrounding whitespace from the input and each set item,
preserve case, and canonicalize sets by sorting and deduplicating names.
Whitespace-only input SHALL mean no rights. Explicit names SHALL follow the
runtime concrete namespace grammar and classification; valid names need not
currently exist. Empty set items and wildcard mixed with names SHALL be invalid.
Persisted peer records SHALL use the same parser as provisioning and updates;
malformed records SHALL fail store loading rather than acquiring default or implicit rights.
Canonical persistence SHALL use `*`, comma-separated names, or absent scope.
The application introspection scope model SHALL remain separate and unchanged.

#### Scenario: Explicit namespace set

- **WHEN** peer scope is `beta, alpha,beta,GLOBAL`
- **THEN** its canonical scope is `GLOBAL,alpha,beta`
- **AND** it covers all addressable principals in those namespaces only

#### Scenario: Whitespace is trimmed before canonicalization

- **WHEN** peer scope is `  alpha , beta `
- **THEN** its canonical scope is `alpha,beta`

#### Scenario: Wildcard includes present and future addressable namespaces

- **WHEN** a peer holds `scope="*"`
- **THEN** GLOBAL and every current addressable namespace are covered
- **AND** a bundle or newly introduced namespace type becoming addressable is
  covered immediately without changing the credential, grant or connection

#### Scenario: No scope grants nothing

- **WHEN** scope is omitted at registration or is empty/whitespace-only
- **THEN** the principal has no ingress rights

#### Scenario: Validate namespace syntax independently of existence

- **WHEN** a set names a syntactically valid future namespace
- **THEN** registration or update accepts that name without requiring a catalog
  entry
- **AND** malformed set items or wildcard mixed with names fail validation

#### Scenario: Application rights do not expand

- **WHEN** peer namespace scope support is enabled
- **THEN** application introspection and its snapshot/revocation filtering
  retain their own existing scope model

### Requirement: Authoritative Peer Scope Updates

The relay SHALL support in-place replacement and clearing of a registered
peer's scope under the dedicated scope-administration permission. The update
SHALL preserve the PSK and its stored hash, principal identity and type, expiry,
and all unrelated record metadata. It SHALL NOT disconnect the peer or emit
credential revocation events for a scope-only change. PSK rotation SHALL
preserve the latest committed scope rather than widen or restore an older one.

Every ingress delivery and discovery authorization SHALL consult the current
authoritative record for the authenticated peer, not a Hello-time grant
snapshot. A missing or unreadable authority SHALL NOT authorize. Reuse of a
principal id after drop or re-registration SHALL NOT make an obsolete credential
binding current. Request-supplied scope and `on_behalf_of` SHALL NOT authorize.

Scope replacement SHALL be serialized with other principal-store mutations
and final ingress authorization decisions through a store commit. The atomic
rename SHALL be the update linearization point for effective authority;
durable success SHALL additionally require parent-directory sync before
acknowledgement. That sequence SHALL be temporary-file sync,
atomic rename publication, then parent-directory sync. The rename SHALL publish the replacement as the effective grant
while the shared boundary is held; directory-sync failure SHALL NOT revert
effective authority to the old grant. Validation, authorization, or failure
before rename SHALL leave the entire old durable and effective record
unchanged. Parent-directory sync failure SHALL produce an
indeterminate-durability error rather than success, SHALL NOT claim the old
grant remains durable or effective, and SHALL report the effective replacement
scope when scope is returned. An idempotent replacement SHALL succeed only
after completing authorization and directory synchronization; same-scope
repetition SHALL NOT short-circuit synchronization and SHALL follow ordinary
last-writer-wins explicit-request semantics. Concurrent replacements SHALL
take effect in commit order, not as a union.

For Send/Raww the grant decision and admission SHALL be ordered together against
scope commits: no operation SHALL authorize under the old grant, allow a scope
commit to intervene, and then admit under the obsolete decision. Every target
must be covered before any is admitted. A request prepared before the update
but not finally authorized/admitted SHALL use the new grant when ordered after
the commit. Already admitted deliveries SHALL NOT be retroactively cancelled.

For discovery the authorization decision fixing the filtered result SHALL be
ordered against the same scope commits. A result fixed before a commit MAY
arrive after update success; a discovery decision ordered after commit SHALL
use the replacement or a later committed grant. This SHALL apply to both
namespace and concrete-namespace principal discovery on existing connections.

After durable update success, all subsequent authorization decisions SHALL observe
the replacement or a later committed grant. Socket response loss after durable
commit SHALL NOT undo the update or be represented as a pre-commit failure.
An indeterminate-durability error SHALL NOT be represented as success or as a
pre-rename failure. Local
scope-update inscriptions SHALL identify the actor, peer and old/new canonical
scope without disclosing credentials.

#### Scenario: Narrow on an existing connection without rotating credentials

- **WHEN** an authorized operator replaces a connected peer's `*` with `alpha`
- **THEN** its connection, PSK, identity, expiry and unrelated metadata persist
- **AND** subsequent Send/Raww and discovery decisions cannot use the old grant

#### Scenario: Update wins a race with delivery preparation

- **WHEN** a request prepares under a grant covering beta
- **AND** a scope update removing beta commits before final authorization/admission
- **THEN** beta is denied and the old preparation does not permit admission

#### Scenario: Admission wins a race with scope update

- **WHEN** a delivery is authorized and admitted before a narrowing commits
- **THEN** it may execute after update success
- **AND** the scope update does not retroactively cancel it

#### Scenario: Discovery decisions straddle a scope update

- **WHEN** one discovery result is fixed before a narrowing commit and another
  decision is ordered after that commit on the same peer connection
- **THEN** the first may still arrive after the update response
- **AND** the second contains only data covered by the replacement grant

#### Scenario: Failed update before publication leaves the old record intact

- **WHEN** validation, authorization or persistence of a scope update fails
  before rename publication
- **THEN** the prior scope, PSK, identity, expiry and metadata remain unchanged
- **AND** subsequent ingress decisions continue to use the old scope

#### Scenario: Directory-sync failure is indeterminate, not old-grant-intact

- **WHEN** rename publication succeeds but parent-directory sync fails
- **THEN** the replacement remains the effective grant
- **AND** the caller receives an indeterminate-durability error reporting the
  effective replacement scope, not success and not an old-grant-intact claim

#### Scenario: Concurrent rotation does not restore stale scope

- **WHEN** rotation and scope replacement run concurrently
- **THEN** they serialize without overwriting each other's fields
- **AND** the resulting record has the rotated credential and replacement scope
  if both commit successfully

#### Scenario: Clear and retry safely

- **WHEN** an administrator successfully sets empty scope and repeats it
- **THEN** both requests succeed without rotating or disconnecting the peer
- **AND** the repetition completes authorization and directory synchronization
  rather than short-circuiting as a no-op
- **AND** the peer has no delivery or discovery rights

#### Scenario: Missing peer authority cannot reuse an old grant

- **WHEN** a peer's record is dropped or its credential binding is superseded
- **THEN** a not-yet-admitted ingress request cannot authorize from a cached
  grant or a newly registered record sharing only its principal id
