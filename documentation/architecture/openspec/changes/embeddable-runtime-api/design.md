## Context

Identity federation is complete and archived as
`2026-06-03-add-identity-federation`, so the relay has stable principal
verification, introspection, revocation, and attribution contracts to build on.

The resolved embedding design is public-API-first: relay protocols are layered
over public handlers, not the other way around. The standalone relay binary is
the first consumer of the embeddable API. A foreign host such as `litrpg` uses
the same public runtime boundary in-process.

## Goals

- Make public dispatch, provisioning, and introspection functions the Rust
  embedding boundary.
- Ensure the standalone relay server, socket, MCP, CLI, stdio, and future
  transports call the same public handlers.
- Preserve equivalent relay semantics across embedded, standalone, and sidecar
  topologies.
- Keep caller-supplied identity descriptors distinct from relay-verified
  principal context.
- Carry principal-home and operation-bundle as distinct fields with one
  explicit bare-target fusion rule (decided with Frontend Engineer; authority:
  0.11 standup sequencing adjustment 2).
- Accept caller-configured config/state roots for embedded runtime startup.
- Support `direct_psk` for host-held in-memory credential material.
- Add `Content-Type` as the canonical envelope payload discriminator.
- Specify ACK correlation cleanup so transport-accepted work cannot leave stale
  pending state when `accept_ack` never arrives.
- Provide a coherent delivery execution, observation, and lifecycle boundary
  for embedders, alternate hosts, diagnostics, and deterministic test harnesses.

## Non-Goals

- No extension registry or extension discovery surface in this proposal.
- No MCP poll surface for extension events in this proposal.
- No application-domain authorization requirements.
- No cross-relay trust or discovery topology.
- No public contract for private socket wire frames.
- No direct public contract for worker-registry entries, internal delivery-task
  fields, worker-close functions, or terminal-outcome completion functions.

## Decisions

- Decision: the embeddable boundary is public typed relay handlers and typed
  request/result structures, not a transport adapter trait.
- Decision: the standalone relay binary embeds the runtime by constructing the
  same public runtime object and calling the same handlers that foreign hosts
  call.
- Decision: transport adapters own framing, serialization, connection liveness,
  and stream lifecycle only.
- Decision: public dispatch takes relay-verified principal context. Caller
  identity descriptors and credentials are input to verification/provisioning,
  not authorization context.
- Decision: embedded runtime initialization accepts caller-configured config and
  state roots; Agentmux owns its internal layout beneath the configured state
  root and does not assume a fixed parent layout.
- Decision: the public identity descriptor credential-source vocabulary includes
  `direct_psk` so embedded agents can authenticate with host-held in-memory PSK
  material.
- Decision: private Hello/request/response frames remain socket protocol
  details.
- Decision: `Content-Type` discriminates envelope payload semantics while
  keeping `text/plain` behavior compatible with existing message delivery.
- Decision: content types that require `accept_ack` must have a bounded timeout
  and must clear pending correlation state when the timeout fires.
- Decision: the runtime owns delivery task construction, delivery execution,
  worker registration, receipt generation, correlation, and shutdown gating in
  every topology. There is no public executor, transport, or worker injection
  point; external delivery extension belongs at the versioned executable
  agent-adapter wire boundary of the follow-up adapter contract. Embedders
  observe typed delivery lifecycle outcomes through the public observer and
  drive determinism in tests through the existing harness techniques against
  that observation boundary, not through an injected implementation.
- Decision: principal-home and operation-bundle are independent axes carried
  as distinct fields in the identity descriptor, the verified principal
  context (explicit home field set once at verification), and dispatch inputs
  (explicit operation-bundle parameter). Fusion happens at exactly one rule:
  explicit target suffix > operation-bundle parameter > bundle-scoped
  principal-home > reject for relay-wide principals. Descriptor routing
  defaults define operation-bundle defaults only, never identity.
- Decision: the public runtime handle supports ordinary handler dispatch and a
  controlled shutdown lifecycle. Dispatch and observation remain at the runtime
  contract level; callers do not directly enqueue internal tasks, mutate the
  worker registry, close individual workers, or complete outcomes.
- Decision: credential-backed verified principal context is opaque
  and runtime-issued, constructible only on Agentmux verification or
  authorized provisioning paths. Authority is frozen to the
  credential/principal binding epoch and bound to the issuing
  runtime/identity-store instance: cross-instance presentation is
  rejected, rotation/revocation/deletion invalidates outstanding
  contexts, recreation starts a new epoch, and dispatch after
  shutdown begins is rejected. Binding epoch is not
  delivery-consumer generation. Provisioning mints through the
  authorized path only; read-only introspection never mints dispatch
  authority of either branch. Registration-minted input follows the
  authorized registration rule instead: transport admission mints it,
  bound to instance plus connection and registration generation,
  with the same handler surface and per-operation authorization.
- Decision: two simultaneously live handles with separate roots, state,
  registries, ledgers, and shutdown gates are fully isolated — one's
  shutdown, delivery, and observation never affect the other, and neither
  accepts the other's contexts or sees the other's provisioned state.
- Decision: broadcast requires the explicit operation-bundle parameter for
  every principal kind, with no principal-home fallback. Fan-out scope is
  never inferred.
- Decision: `text/plain` lifecycles terminate at transport submission with
  existing guarantees unchanged. For accept-requiring Content-Types,
  `transport_ack` is a stage disposition and the lifecycle terminates at
  accept or timeout, exactly one terminal per lifecycle, with ack/timeout
  admission serialized through the correlation owner. Submission evidence is
  three-way — observed `transport_ack` proves submission, observed provable
  transport failure proves non-submission (distinct no-effect terminal),
  otherwise submission-unknown (distinct terminal, no timeout, no blind
  replay). An `accept_ack` admitted before its `transport_ack` is honored as
  the stronger evidence and records the accepted terminal.

## Risks / Trade-offs

- Public handler boundaries may expose implementation seams that were previously
  private. Mitigate by publishing typed operation/context types rather than
  private frame structs.
- Refactoring the standalone relay to consume the new API may be larger than a
  foreign-host-only library wrapper. This is intentional: two relay semantics
  paths would drift.
- `Content-Type` support creates space for Proposal B but does not by itself
  define extension registration. Keep extension-specific behavior out until the
  follow-up proposal.
- An injectable executor would become a test hook disguised as API mirroring
  private worker machinery. Resolved by removing the injection point: the
  contract exposes resolved delivery outcomes for observation, and
  determinism in tests comes from harness techniques against that boundary.

## Migration Plan

0. Sequencing prerequisite (not part of this change): `todos/backend/2`
   (process-global singleton removal) lands first per the operator's
   2026-08-25 decision, so the public runtime handle can own registry,
   ledger, configuration, and shutdown state per instance instead of per
   process.

1. Introduce public runtime state/configuration types and public handler
   functions for existing relay operations.
2. Move standalone relay socket handling to frame/decode requests and call the
   public handlers.
3. Move MCP, CLI, and stdio surfaces to call public handlers directly or through
   thin clients that preserve the same handler contract.
4. Add public identity descriptor, verification, provisioning, verified
   principal context, root-configuration, and `direct_psk` credential-source
   types.
5. Add `Content-Type` to the canonical envelope model with `text/plain` as the
   default/current message behavior.
6. Add ACK pending-state timeout cleanup for content types that require
   `accept_ack`.
7. Add parity tests covering standalone socket use and in-process handler use.
8. Add a typed delivery lifecycle observer and controlled runtime shutdown
   contract without publishing worker-registry internals or any execution
   injection point.
9. Add deterministic tests that assert delivery outcomes and shutdown races
   through the public observation and shutdown boundary.

## Phase 2 — Public dispatch surface (tasks 1.2/1.5/1.6; tests 2.1/2.2/2.5-adjacent)

Public dispatch handlers take `(&RelayRuntime, &VerifiedPrincipal,
operation)` and return typed results or typed relay errors. No handler
takes wire identity, connection bindings, or bundle paths as authority.

### Boundary conversion (exactly one function)

One function maps the internal connection binding plus bound bundle to the
sealed dispatch input, freezing `home_namespace` there, with two
branches and one handler surface. The store-backed branch keys on
authenticated identity only: never on the wire `namespace`
parameter, never on admitted-but-unauthenticated identity. The
transport branch freezes identity and home from the admitted
registration and bound configuration, never from wire per-request
namespace, without elevating the admitted identity to authenticated
standing. The `RELAY`-home override for peer ingress is the reference
pattern. No per-call-site conversions, no public constructor, no
alternate dispatcher.

### Threading and final admission

The same `&VerifiedPrincipal` object flows from the single
`check_context` call through route resolution into admission. Reusing
the object is reusing neither the grant nor the initial validity
proof: at final admission, inside one lock span held through commit
or rollback of the whole local admission, the handler revalidates
BOTH the original context branch-specific validity AND the current
kind-specific membership and authority for the entire resolved
route. A scope narrowing that preserves the credential epoch still
denies, because the decision reads current grants, not the earlier
proof; a rotation that preserves grants still denies, because the
credential binding epoch is revalidated against the live store and
the old epoch is stale. The credential-free path enforced TODAY is
the legacy socket path (pre-D1 behavior): it carries no store
binding, so the span revalidates membership and grants for it but
no epoch — connection liveness stays transport-owned (a dead
connection cannot dispatch at all). That legacy description is NOT
the required registration-minted branch: per the landed D1
amendment (spec section below) the registration-minted identity
carries its own registration/connection-generation epoch,
intentionally separate from the credential-store epoch, with
lifetime invalidation on replacement, reconnect with a raised
generation, and bound-configuration change. Enforcement of that
epoch in the send span is REQUIRED but not yet implemented here —
an open obligation with a future invalidation-test obligation,
neither deleted nor rewritten as liveness-only. Peer relays
re-resolve their live ingress scope from the
current store record under the held identity-admin serialization.
Synchronization ownership, stated exactly: the store transaction
lock serializes credential and peer-scope invalidation ONLY —
provision, rotation, and revocation commit under that lock, which
the span holds. It does not serialize bundle or policy file
invalidation, which takes no lock; nothing in this section implies
otherwise. Admission observes lock-free writes by reloading fresh
state under the span and re-resolving the sender and the whole
route against it, so a write landing before the final read denies
(the narrowing and membership-removal traces prove it). A write
landing after the final read is observed on the next operation; that tail
is carried open below, not implied away. There is no check-then-unlock and reacquire before
publication: a validity check outside the protected span would
recreate the gap. The design test matrix parks dispatch after
initial context validation, then invalidates a credential binding
while grants stay unchanged and narrows or removes membership and
configuration while grants stay unchanged, requiring denial
with no partial admission; the converse admitted-before-invalidation
outcome is preserved alongside the existing narrowing and shutdown
traces.

### Shutdown and admission linearization

Shutdown is an independent atomic gate, not serialized by the store
lock, so re-reading it under that lock alone closes nothing: two
reads merely shorten the window unless the gate read itself is a
defined linearization acquisition that shutdown must observe. The
handshake is an explicit per-instance admission/shutdown guard,
separate from store and grant serialization:

- Admission holds the guard across the whole check-to-commit span:
  acquire guard, read gate, revalidate the credential binding, pass
  the pre-commit gate, re-resolve grants and membership from fresh
  state, authorize, reserve quota, pass the reserved gate, publish
  each reservation to the local queue and worker registry, release
  guard. Reservation and publication are one atomic commit against
  shutdown: an enqueue failure rolls back every reservation with no
  task, so each admission is either queued or released. Only
  cross-relay forwarding runs after release, since peer transport
  execution must never sit under the shutdown gate or the store
  lock. Publication always precedes release.
- Shutdown publishes the gate first, then waits on the guard before
  declaring the admission barrier complete. `begin_shutdown` proves
  that barrier released; it does not prove workers drained. The
  serve path drains after the barrier in order: connection-worker
  drain, then async-delivery drain (`wait_for_async_delivery_shutdown`
  under the process-global flag), then per-bundle teardown. In-test
  shutdown evidence therefore claims barrier ordering only —
  waiter arrival while the span is held, barrier held while a
  reservation is unpublished, release after publication — and the
  reserved-gate trace additionally correlates the queued
  inscription with the returned message id, proving publication
  executed under the span. Worker-drain-to-zero belongs to the
  serve-path sequencing and is not claimed by the admission tests.
- Ordering closes the interleave both ways. Admission acquiring
  first reads gate-clear, reserves, publishes, and releases; the
  shutdown wait then observes the published reservation, and the
  serve-path drain that follows sees published queue entries: no
  accepted-but-invisible worker. Shutdown publishing first makes
  the in-span gate read refuse: no publication after the barrier.
  The store lock orders grants; the guard orders the barrier.
  Neither is held over transport or network execution;
  admitted-before-shutdown work runs to the serve-path drain
  semantics.
- Lock order, outermost first, with one documented per-path exception.
  Ordinary dispatch acquires guard, then store, then ledger and
  registry; peer-ingress dispatch holds the identity-admin
  serialization across its live grant binding before reaching the
  span, giving admin, then guard, then store for that path only. No
  path takes the admin lock while holding the guard or the store
  lock, and no path takes the guard while holding the store lock, so
  the exception introduces no cycle; it is retained because the held
  admin lock is what keeps the ingress binding current. Shutdown
  publishes the gate and waits on the guard holding no inner locks,
  so the wait can never wedge against an admission holding the store
  lock; the drain takes registry and worker locks only after the
  wait completes, which is why work published inside the span is
  drain-visible and no worker spawns after the drain.
- Residual window, carried open. The revalidate-to-publication
  passage covers lock-free file writes (policy and bundle files)
  that take no span: admission reloads them fresh under the span,
  and every dispatch re-observes on entry, so a write landing
  before the final read denies and a write landing after it applies
  to the next operation at the latest. That bound is descriptive,
  not a linearization contract: a test seam is not a public
  linearization point, and no claim here weakens the final-admission
  rule. Closing the tail entirely needs writer-side cooperation (a
  managed config publication boundary or generation epoch with
  external edits effective at publication), which requires an
  explicit proposal, operator direction, and a normative delta — it
  is not approved here and is not implied by this change.
- The design test matrix parks the interleave at each boundary —
  gate read, epoch revalidation, pre-commit gate, grant
  re-resolution, reservation, publication — asserting refusal or
  commit-then-barrier-released, never orphaned or missed. The
  reservation and publication boundaries landed with the reserved-gate
  trace (arrival-proved wait, held exclusion, queued-inscription
  correlation); the race is not claimed closed until the remaining
  matrix lands with the mechanism.

### Membership in dispatch

`verify_identity` stays membership-free: host-provisioned means
host-responsible, and the responsibility sits on the dispatch path.
Membership wording is principal-kind specific. Bundle-session
principals prove membership in their home namespace. Relay-wide
users and applications follow their configured operator authority,
and relay peers follow live ingress authority — neither borrows an
operation bundle's membership nor fails for lacking a bundle. Every
public dispatch handler enforces the path matching its caller kind;
`dispatch_list` gains a verified-context authority slot instead of
resolving the sender from a wire string, and the zero-auth
global-list bypass is closed or removed.
Application scope is preserved, not widened and not migrated. Operations
gated on application-specific scope or capability keep those exact
rules at their current enforcement points: the boundary conversion
carries the application's recorded scope alongside the context, and
handler extraction must not substitute generic operator controls or
bundle membership for them. The Phase-2 test matrix pins an
application principal authorized for its narrow scope and denied
outside it, byte-identical to the socket path.

### Bare-target fusion and broadcast

Fusion moves server-side with one rule: explicit target suffix >
operation-bundle parameter > bundle-scoped principal-home > reject for
relay-wide principals. Every dispatch input carries the explicit
operation-bundle parameter. Broadcast requires the parameter for every
principal kind with no home fallback; fan-out scope is never inferred.

### Socket as framing adapter (1.6); host as first host (1.5)

Framing keeps codec, Hello/binding, stream registry, writer tasks, and
the pool hop. Everything from request routing down moves behind the
public handlers. The standalone host constructs `RelayRuntime` and
calls the public handlers instead of building its own delivery state.

### Cutover gate (enforceable non-operative boundary)

The public send surface is non-operative by construction, not by
convention. `dispatch_send` and `SendOperation` exist only under the
`experimental-public-send` cargo feature; in default builds the
symbols do not exist and no embedding host can name the path. There
is deliberately no runtime opt-in switch to build: a host-writable
config knob would be host-reachable enablement, so no such knob
exists until the cutover gate opens. The socket transport never
passes through the gated entry, so socket dispatch is unaffected in
every build configuration; the standalone host's future migration to
the public handlers (1.5) follows the same cutover.

The gate opens when either the managed-publication contract lands
(proposal (a): external edits effective at a managed publication
boundary or generation epoch, admission validating that generation
under shared serialization — operator pre-review, then Tier-1/Tier-2)
or the operator explicitly chooses the weakened-semantics path (b)
with its own normative amendment and review. A runtime operator
switch, if one survives to cutover, is activation AT that point on
those terms — never approval of a specification change, and never
built ahead of it. Until then the operative public-send cutover
stays held, and no packet claims contract completeness.

Validation runs both configurations: default (surface absent;
socket suites prove no regression) and explicit experimental
(`--features experimental-public-send`, full public-send suite).
The experimental suite is the cutover evidence, not a second
implementation: one shared spine, no socket fork — the socket
suites run green in both configurations as the parity proof.

Dispatch wiring binds the shared pair to its owning domain. The
public entry takes a `DispatchWiring` instead of a bare catalog and
peer manager: the handle carries the domain's live catalog and peer
manager stapled to the domain's delivery Arc, constructed only by
`live` (reads the installed pair, fail-closed when absent) or
`mirrored` (adopts the installed pair if present, else builds from
the runtime's own roots and installs). No constructor accepts a
caller-supplied pair, so a foreign catalog can never be wrapped;
dispatch refuses with `validation_wiring_runtime_mismatch` unless
the wiring's delivery `Arc::ptr_eq`s the dispatching runtime's.
The pair lives inside the delivery state itself, so it is released
exactly when the domain drops — a reclaimed root starts from an
empty slot by construction and stale generations cannot linger.
First installation wins per domain generation while the installer
still owns it, and the first installation must also agree with the
configuration identity already bound to the owning delivery: the
ordered, resolved layers are bound BEFORE the claim publishes the
delivery as an adoptable domain (runtime construction, claiming
prepare, and serve construction all bind their candidate first), so
no concurrent adopter can ever observe an unbound owner. A mismatch
is refused with a configuration-mismatch error before anything
installs or starts, and an install against a somehow-unbound owner
fails loud rather than accepting an arbitrary installation. A live
owner never rebinds; a restarted domain binds anew once all prior
holders release. This is a per-generation source-identity binding,
not a permanent deployment-to-configuration mapping. Concurrent
mirrored calls converge on the installed pair instead of forking
it; a different pair over a committed one is refused, while a
different pair over a still-tentative one fails with an initializing
error so the loser retries after the winner commits or aborts. `prepare` installs its pair tentatively right after the domain
claim (after the principal-store prune, so prune errors need no pair
cleanup) and before any bundle startup side effect -- the reservation
IS the preflight, so a domain that already hosts a pair fails there
with a conflict, before workers spawn or listeners bind. The tentative
install is unreadable: live/mirrored reads fail closed with an
initializing error until the commit boundary publishes the pair, so
the reservation grants no operative send authority by itself and no
wiring for a pair that may yet abort can ever be issued. The commit
boundary sits after every fallible step (bundle startup, principal
registration, listener bind, serve construction): publish, defuse the
abort guard, serve. That boundary -- not entry intents, not promotion
timing -- is the readiness definition, so per-route bypass questions
are moot: nothing is dispatchable before it. Bundles that fail to
start are removed from the installed catalog in place before publish,
and the serve context reuses the early-installed identity, so
registration, failure pruning, and serving can never diverge onto
different objects. Any post-install exit that does not reach the
commit boundary unregisters the tentative pair via a drop-guaranteed
abort guard (defused only on success), so a failed preparation leaves
no retained pair behind and the next preparation over the same live
domain starts from an empty slot instead of conflicting with its
predecessor's residue. Because no wiring is issuable before publish
and committed pairs are never unpublished, no stale or aborted wiring
can exist to revoke: retention across abort is unconstructible, and
the paused-window and abort traces prove the fail-closed reads rather
than post-success state. A state-root-filtered test gate parks
preparation between the tentative install and the first bundle side
effect, pinning the unpublished window with a timing-observable trace.
Read-only `catalog()`/`peers()` views expose the bound pair without
conferring rebinding authority.

Scope treatment of the shared spine and shutdown changes, proposed
for agreement: the admission guard, publish-inside-span span, commit
and reserved gates, and shutdown hooks live in dispatch code paths
the socket transport also executes, so they ride as socket-visible
improvements under routing parity rather than held-back scaffolding.
They are covered by the full suite including the socket suites in
both configurations and are required for the final shape; forking
the spine for the hold would create the very divergence Phase 2
exists to remove. The domain-pair slot lives inside the owning
delivery state and exists only under the experimental flag: default
builds have no slot and no production write path to it. The
process-global map holds only weak delivery handles for the
state-root claim plus the pair mutex ordering (domains-then-pair).
The wiring constructors, the dispatch brand check, the
tentative/publish/unregister calls, and the prepare abort guard all
sit behind the experimental flag with the slot.

### Registration-minted authority (Coordinator D1 ruling, adjudicated)

Credential-free socket sessions are authorized by registration minting,
not by verification: transport admission as an authorized
administrative mutation produces the dispatch input, bound to the
runtime instance plus the connection and registration generation.
The exemption alternative is the carve-out class the frozen contract
forbids. Minimal contract:

- Issuer: the serving transport at connection registration. Never
  verification, provisioning, or caller construction. Opaque and
  internal; no public trust sentinel.
- Scope: Session and User principals admitted with enforcement off
  only. Application and Relay principals always require a recognized
  token; no credential-free branch exists for them.
- Lifetime: invalidated by live-claim replacement, reconnect with a
  raised generation, disconnect, shutdown, and bound-configuration
  change. No lingering contexts, no grace reuse.
- Authority: none of its own beyond the admission. Authorization runs
  per operation in the requester home namespace exactly as for any
  other route, with no scope-threshold exemption.
- Attribution: truthfully non-store-authenticated in every local
  record, diagnostic, and attribution. Admitted never renders as
  authenticated; no elevation either direction; no new wire marker.
- Effects: no store credential created, no other store side effect.
  A fresh registration is a separate mint, never an in-place
  provenance upgrade or downgrade.

### Admission taxonomy (settled with tool-ops)

Relay-authored denials enter the canonical operation error set with
code and details and pass through unchanged: per-target quota,
payload-too-large, session-type refusal, ACP queue-full and
worker-unavailable, startup failures, shutdown, and all routing and
authorization codes. Adapter-only exclusions stay out: transport
timeouts, unavailability, connection-limit, and unassociated-server —
never originated as relay errors from dispatch.

### Interleaving evidence (re-examined at this dispatch)

Admit-gate tests land with the mechanism: park session dispatch at the
authorize-to-admit seam, commit revocation, narrowing, or shutdown
mid-pause, and assert denial with no partial admission plus survival
of admitted-before-commit work. Store-level seam interleavings are
explicitly re-deferred at this gate with justification, not rolled
forward silently: the store span is unchanged since the Phase-1
evidence (mutex held across the full sequence at every mutation
site, lock-removal production mutation failing exactly the race
tests), while Phase 2 introduces a new dispatch surface whose
authorize-to-admit seam has no deterministic coverage today — the
scarce testing capacity goes to the new risk. The validation section
names the store gap as uncovered and tracked at issues/general/8.
Acceptance received from the Coordinator at this gate, with
conditions carried in the issue: the validation section names the
gap; the issue records the gate evidence and re-examines at the
Phase-2 merge gate; the deferral lapses automatically if
implementation touches lock or handoff structure inside the span.

Scope note at the public-send slice: the final-admission span IS new
lock and handoff structure inside the store hold (the admission
guard, plus ledger reservation and registry publication moved under
the store mutex), so the issues/general/8 deferral lapses for that
span — it is not carried as still-deferred. What stands unchanged
is the identity-store transaction boundary itself (provision,
rotation, and revocation still load, mutate, and persist under the
same mutex at the same sites); its Phase-1 evidence is unaffected.
What justifies the extended store hold is stated in the shutdown
section above and verified, not assumed: nothing under the extended
hold blocks on transport or network (in-memory lookups, unbounded
channel sends, spawns, one O_APPEND inscription per target), and the
acquisition order store, then ledger and registry has no inverse
path. Seam-level interleaving evidence for the new span structure
remains open and is re-examined at the Phase-2 merge gate.

### Binding constraints carried

Membership in dispatch; provisioning authority and dispatch authority
never conflated; check once per operation and reuse across a
multi-target route; current authority at final admission under the
transaction boundary; no public dispatch route bypasses the context
check.

## Open Questions

- Exact public Rust type names are implementation details as long as the
  caller-supplied descriptor and relay-verified context remain distinct.
- Exact `accept_ack` timeout duration is an implementation/configuration detail,
  but timeout behavior and stale-correlation cleanup are normative.
