# Design delta 001: struct-held operation context (PROPOSED v3)

Status: PROPOSED v3 — supersedes v1 and v2 texts in full. Operator
pre-review authority must be re-granted (prior grants covered prior
texts); Tier-1 scope/text sign-off PENDING; required Tier-2
architecture/security review of the ownership/construction contract
applies before final scope approval. No wiring proceeds on this
text. On sign-off it folds into `design.md` Public API Shape /
Boundary Design (§1.4), `tasks.md` 1.4 + 2.x notes, `proposal.md`
boundary wording, and `specs/tool-operations/spec.md`, and this file
is removed.

## Supersedes (narrowly)

- `design.md` Public API Shape, 4th inclusion bullet (async operation
  functions over runtime handle, principal context, typed input).
- `tasks.md` 1.4 (operation function signatures over the public
  runtime handle and relay-verified principal context).
- `specs/tool-operations/spec.md`, Canonical Tool Operations API
  requirement: "public typed Rust functions" and "SHALL NOT require a
  host or server object", as applied to the operation-state handle
  (clarified below, not removed).
- `proposal.md` Dependencies wording ("Operation signatures take the
  public runtime's relay-verified principal context ...") and the
  Non-Goals boundary rule ("No MCP-specific host, server, or
  association type as the public embedding boundary"), as applied to
  the operation-state handle (distinguished below, not weakened).

Everything else in those artifacts stands unchanged, including the
wave-1/wave-2 publication split, BundleScope semantics, the
validation/authorization/error rules, and the R1 managed-publication
cutover hold.

## New normative text

For each operation-backed tool there is one canonical operation
object. It is a struct holding the public runtime handle
(`Arc<RelayRuntime>`) and the sealed dispatch wiring defined below,
with one async invoke method taking the relay-verified principal
context and the typed input, returning the typed output or the
canonical error:

```rust
pub struct SendOperation { /* illustrative only */
    runtime: Arc<RelayRuntime>,
    wiring: DispatchWiring,
}

impl SendOperation {
    pub fn new(
        runtime: Arc<RelayRuntime>,
        wiring: DispatchWiring,
    ) -> Self;

    pub async fn invoke(
        &self,
        principal: &VerifiedPrincipal,
        input: SendInput,
    ) -> Result<SendOutput, OperationError>;
}
```

Implementation may choose exact names; the boundary constraints in
Public API Shape govern, extended to the struct: operation struct
fields and constructor parameters carry no MCP, rmcp, JSON-RPC,
stdio, or JSON schema types, and no private relay socket frames;
optionality stays in the type system.

### Why a struct (tradeoff rationale)

The alternative — a free async function taking the runtime handle,
principal, and input — would have to thread the shared catalog and
peer manager on every call: either as extra parameters (widening the
public signature at every call site and re-asking, per call, which
pair is live), or by rebuilding them per call (which reintroduces
the Tier-1-named defects: default intent, unshared peer sessions,
file-existence authority inference), or by stashing them on the
runtime (a second source of truth forked from watcher-mutated live
state). The struct records the once-per-context binding in the type:
construct once against the owning runtime, invoke many times, with
the wiring shareable and checkable. This is a tradeoff judgment for
stable signatures and checkable sharing, not a claim that a
reference to shared state is inherently a second source of truth.

### Operation-state handle vs host/server object (normative)

An operation-state handle is a capability bundle, not a server: it
owns no transport endpoint, resolves no association, generates no
schemas, serves no connections, and cannot act without a
per-invocation verified principal. It holds shared handles to
runtime-owned relay networking (the delivery context, catalog, and
peer connections) for dispatch — that is access to the owning
runtime's machinery through the sealed wiring below, not a second
transport and not a hosted dependency. The "no host or server
object" rule targets `McpServer`, `ToolRouter`, transports, and
association-bearing hosts — objects that authorize, frame, or
present. The struct holds none of those capabilities, so it is not
a host/server object under this rule.

### Construction (one live pair, per context)

The operation struct constructor takes `(runtime, wiring)` and holds
both; no catalog or peer manager is ever rebuilt from files or
copied as a snapshot inside the operation path. The wiring itself is
sealed (next section): only its two constructors can mint it, and
neither accepts a caller-supplied pair.

- Adapter (post-3.1): `DispatchWiring::live` against the serve
  context it already holds (clones the registered live pair).
- Embedded callers: the Backend once-per-runtime constructor slice
  (returns the live pair; see contract below).
- Tests: direct construction via `DispatchWiring::mirrored`, the
  same path any embedded host uses — no test-only constructors
  exist, so no provisional bypass can become supported.

Backend wiring contract, quoted verbatim with attribution (Backend
Engineer, 2026-11-14; normative reference until the constructor
slice lands, which carries the code). This supersedes the v1/v2
mixture rule and the peer getter entirely:

> DOMAIN-BOUND REGISTRATION. The domain registry gains a pair slot
> beside the delivery Weak, same key, same lifecycle: released
> together in release_domain_slot, never separately. Registration
> rule: ConnectionServeContext construction does NOT register
> anything (adopt path behavior unchanged — zero socket-test blast
> radius); the host prepare path explicitly calls
> register_domain_pair(delivery, state_root, catalog, peers) once
> after building the serve context, and mirrored() registers on
> first use. Register requires proof of ownership — the passed
> delivery Arc must ptr-match the domain's upgraded Weak or it
> fails — then first-writer-wins per domain generation;
> re-registration with identical Arc identity is idempotent Ok; with
> different content it is refused (mid-generation replacement
> requires domain regeneration). The V2-1 counterexample is
> defeated structurally: serve(stateRootA, catalogB) on an
> init-claimed A registers nothing (only prepare/mirrored register,
> and this serve call is neither), so wiring::live(runtimeA) finds
> an empty slot and fails closed with
> validation_wiring_unregistered — pairB is never consulted, never
> yielded.
>
> SINGLE ISSUANCE. mirrored(runtime, peers, autostart): if the slot
> holds a pair, it adopts the registered pair and discards its fresh
> build; if empty, it builds from that runtime's own roots,
> installs, and returns it. live(runtime) (serve-side; takes NO
> serve context): returns the registered pair or fails closed on
> empty/stale. Two mirrored() calls with distinct pairs therefore
> yield identical content — the second adopts. Same-root
> reconstruction in a fresh process is a fresh domain and binds its
> fresh delivery. The dispatch-time brand check stays as defense in
> depth: experimental dispatch_send takes the wiring instead of the
> bare pair and refuses with validation_wiring_runtime_mismatch
> unless Arc::ptr_eq(runtime.delivery_context(),
> wiring.delivery). Stale pre-release wirings fail this check after
> drop-and-reclaim, which is the specified old-context
> invalidation.
>
> RELOAD DOCTRINE (specified, not deferred). Catalog identity is
> shared-mutable: the registered object IS prepare's live catalog
> (same Arc), so watcher-driven bundle load/unload flows through
> every cloned handle with no action and no staleness possible. Peer
> scope is snapshot-at-issuance per wiring; a peer-set change is a
> new manager is a new domain generation (in practice process
> restart, matching current behavior — nothing regresses). Snapshot
> lifetime equals domain generation; invalidation equals the brand
> check.
>
> DROPPED FROM V2: the peer-manager getter (live() no longer takes
> serve, so nothing to get) and the operation-side root-string
> comparison (replaced by brand check plus construction
> discipline). UNCHANGED: refusal-test placement with my constructor
> slice (non-member bundle target refused via
> validation_unknown_bundle; the test now also covers the exact V2-1
> counterexample — foreign-serve construction yields
> error-not-pairB — plus still-pairA-after-second-serve plus
> cross-runtime brand refusal); the quoted Hold/Run intent rule; S2
> model and heartbeat ownership.

Unloaded-bundle refusal coverage rides with the Backend constructor
slice (non-member bundle target refused via the existing
`validation_unknown_bundle` path — pins no-probing and no
Run-defaulting, plus the V2-1 counterexample coverage above).
Intent rule the constructor applies, quoted for citation: a bundle
whose configured membership has autostart disabled, or a relay
started with autostart disabled, enters the catalog as
`HostingIntent::Hold`; otherwise `Run`.

### Per-invocation principal, validated after scheduling

The struct holds no principal, grant, or authorization outcome. Every
invoke takes a fresh `VerifiedPrincipal`, and verification against
the owning runtime (`check_context`: binding epoch, instance
binding, shutdown gate) runs inside the dispatched work — after
scheduling, never cached from a prior call. `spawn_blocking` owns
cloned context, principal, and input; no lock is held across an
await (the struct holds no locks; catalog reads are short scoped
guards inside the blocking dispatch). A panicked blocking task
observed via the awaited join surfaces as
`internal_unexpected_failure`; caller cancellation (dropping the
invoke future) detaches the waiter — the blocking dispatch may still
complete, but no result can be delivered to a dropped receiver, and
none is implied. There is no recall mechanism. The heartbeat test
proves executor progress while forwarding blocks on an
unresponsive peer.

## Reconciliation notes (reviewer conditions mapped)

1. Complete contract amended: Public API Shape bullet replaced (see
   above); tasks 1.4 reworded to struct construction + invoke;
   spec Requirement 1 reworded to operation objects with the
   handle/server distinction stated normatively; proposal boundary
   wording clarified as above. Waves, BundleScope, error and
   validation rules untouched.
2. ONE host-live shared pair: domain-bound registration with the
   delivery Weak sharing key and lifecycle; first-writer-wins per
   generation; only prepare/mirrored register (serve construction
   registers nothing). Masquerade cases defeated structurally:
   foreign-serve construction finds an empty slot (never consults
   the foreign pair); same-call repeated construction adopts the
   identical pair; same-root successors are fresh domains.
   Unloaded-bundle refusal test rides with the Backend constructor
   slice.
3. Principal per invocation; validate-against-owning-runtime after
   scheduling; spawn_blocking ownership; typed join failures
   distinguished from caller cancellation; truthful (no-recall)
   cancellation; heartbeat test with the operation. Reload follows
   the specified doctrine (shared-mutable catalog identity;
   snapshot-at-issuance peers; invalidation equals the brand
   check).
4. Default-off experimental boundary, R1 cutover hold, runtime
   isolation, and the no-protocol-types rule (fields and
   constructor included) are preserved verbatim. Scope approval of
   this shape waives nothing and closes no S1/S2 implementation
   evidence.
