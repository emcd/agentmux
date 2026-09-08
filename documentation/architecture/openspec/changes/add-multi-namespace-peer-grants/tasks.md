## 1. Scope Model

- [x] 1.1 After a separate implementation dispatch, implement one peer scope
  parser for `*`, comma-separated namespaces and empty/no rights, with canonical
  trimming, sorting and deduplication. Use runtime namespace addressability,
  including GLOBAL and future addressable types; add no legacy parser or store
  migration. Keep application introspection scope matching separate.
- [x] 1.2 Validate persisted peer scope with the same grammar and use the
  existing scope field for canonical persistence. Fail closed on malformed
  state without defaults, conversions, or dual-format compatibility.

## 2. Live Authority and Atomic Updates

- [x] 2.1 Replace Hello-snapshotted peer grant authority with current record
  lookup on each ingress operation. Preserve authenticated credential binding
  so missing, dropped or superseded records cannot use stale connection rights.
- [x] 2.2 Implement scope-only record replacement under principal-store
  serialization, preserving credential/hash, identity/type, expiry and
  unrelated metadata/records. Sync the temporary file, publish through atomic
  rename while holding the shared boundary, then sync the parent directory
  before acknowledging success. Pre-rename failure must leave the old durable
  and effective record unchanged; post-rename directory-sync failure must keep
  the published replacement effective and return indeterminate-durability
  uncertainty with the effective scope, never success or an old-grant-intact
  claim. Same-scope retry must reauthorize and synchronize rather than
  short-circuit, following ordinary last-writer-wins semantics.
- [x] 2.3 Serialize scope replacement with new/rotation/drop transactions;
  preserve the latest scope on rotation and other successful field mutations.
  Keep scope-only updates connected, emit no credential revocation event, and
  record actor/peer/old/new scope without secrets. Support clear and no-op.
- [x] 2.4 Order final ingress authorization and local delivery admission with
  scope commits using the shared spine/identity transaction seam. Document
  lock order with the admission ledger; never await or wait for target execution
  under the guard. Preparation alone must not preserve removed access.

## 3. Delivery and Existing Discovery

- [x] 3.1 Apply set/wildcard coverage to all resolved ingress Send/Raww targets
  before any admission, retaining existing existence ordering, origin policy,
  attribution irrelevance, transport gates and chaining restrictions.
- [x] 3.2 Apply current scope to existing namespace and concrete-namespace
  principal discovery under the same commit/decision ordering. Preserve
  nonempty sorted namespace results, covered-absent empty views, out-of-scope
  non-disclosure and local unmatched-scope inscriptions.
- [x] 3.3 Use complete namespace listings and normal diagnostics for peer scope,
  eliminating exact-principal peer filtering without removing the generic
  partial marker from other contracts. Include GLOBAL under wildcard using its
  registry source. Add no discovery aggregation, recipient fanout or operations.

## 4. Administration and Adapters

- [x] 4.1 Add relay ChangeScope request/response and relay-wide dispatch ahead
  of bundle routing; validate peer identity, required string scope and grammar,
  with unknown-principal lookup behind authorization and existing error mapping.
- [x] 4.2 Gate all scope updates with `change.scope=all` through the existing
  action-control map. Add the explicit grant to the starter operator preset;
  leave existing configs and missing-action deny defaults unchanged.
- [x] 4.3 Extend CLI change dispatch with `change scope <principal_id> --scope
  SCOPE`, existing requester/runtime/JSON conventions, canonical scope output
  and vocabulary diagnostics. Keep new-peer string scope and psk-specific
  destinations and behavior separate.
- [x] 4.4 Extend MCP change selector, scope params/validation/handler and help
  catalog/schema. Expose exact `change.scope` args, update new-peer grammar
  descriptions, preserve PSK response contracts and relay error passthrough,
  and add no top-level tool.

## 5. Regression and Race Evidence

- [x] 5.1 Test public interfaces for wildcard coverage of GLOBAL, newly loaded
  bundles and a newly addressable namespace classification; test sets,
  whitespace trimming, canonical sorting/deduplication, future explicit names,
  empty/no rights and malformed input. Verify application introspection
  remains unchanged.
- [x] 5.2 Test scope widening/narrowing/clearing/no-op on one live peer connection,
  proving PSK/hash, identity, expiry and unrelated metadata are unchanged and
  Send/Raww plus both discovery operations use current rights without reconnect.
- [x] 5.3 Add deterministic interleaving tests for update-before-final-admission
  and admission-before-update. Prove no obsolete prepared grant admits work
  after commit and previously admitted deliveries are not cancelled.
- [x] 5.4 Add deterministic discovery races on both sides of scope commit;
  prove later decisions use the replacement while an earlier fixed result may
  arrive later. Cover clearing and simultaneous requests on existing connections.
- [x] 5.5 Fault-inject pre-rename scope persistence failure and verify old
  durable/effective record preservation; fault-inject post-rename
  directory-sync failure and verify the replacement stays effective with an
  indeterminate-durability error carrying the effective scope. Test concurrent
  scope updates, rotation, registration and drop without lost fields or stale
  grant restoration. Cover response loss after durable commit and safe
  idempotent retry that reauthorizes and synchronizes, plus stale
  credential-binding races.
- [x] 5.6 Test dedicated action permissions: missing/none/self/home denied,
  change.scope=all allowed, rotation-only cannot change scope, wildcard peer
  scope is not administration, and unauthorized callers cannot probe record
  existence. Verify starter explicit grants without rewriting existing policy.
- [x] 5.7 Test CLI/MCP/relay parity for provisioning and updates, help catalogs,
  required/unknown fields, clear versus omitted/null scope, canonical output,
  diagnostics and unchanged PSK destinations/revocation/self-rotation behavior.
- [x] 5.8 Test existing discovery filtering: empty/absent namespace cases,
  complete diagnostics and no scope-induced partial marker, unmatched logging,
  one concrete namespace per foreign principal lookup, no implicit GLOBAL
  append, no onward forwarding, and no cross-relay Look or recipient expansion.

## 6. Documentation and Verification

- [x] 6.1 Update relay, commands and MCP subsystem READMEs plus affected nearer
  docs with scope grammar, authoritative operation-time rights, admission/update
  ordering and dedicated action permission. Update usage authorization,
  maintainer configuration, CLI and MCP guides with quoted wildcard examples,
  future addressable-type reach and live scope changes preserving credentials.
- [x] 6.2 Document the intentional breaking peer-grant cutover without legacy
  conversion promises, and response-loss/ordering semantics without claiming
  cancellation of admitted work or retraction of earlier discovery responses.
- [ ] 6.3 Run formatting and standard repository verification plus the affected
  unit/integration and deterministic race tests. Verify MCP help/selector schemas
  through the client after rebuilding; record any refresh or environment issues.
- [ ] 6.4 Run strict OpenSpec validation, delta/rename drop audits and sync
  dry-run. Confirm only the retired exact-principal peer discovery scenario is
  dropped and manually compare all affected normative clauses and artifact
  consistency. Do not sync live specs before the approved archive workflow.
