# Design delta 001: struct-held operation context (PROPOSED v2)

Status: PROPOSED v2 — supersedes the v1 text in full. Operator
pre-review authority must be re-granted (v1 grant covered v1 text
only); Tier-1 scope/text sign-off PENDING. No wiring proceeds on
this text. On sign-off it folds into `design.md` Public API Shape /
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

- Adapter (post-3.1): clones the serve context it already holds
  into the live constructor — serve catalog plus serve peer
  manager. Same staleness semantics as the socket path
  (watcher-driven).
- Embedded callers: the Backend once-per-runtime constructor slice
  (returns the live pair; see contract below).
- Tests: direct construction via `DispatchWiring::mirrored`, the
  same path any embedded host uses — no test-only constructors
  exist, so no provisional bypass can become supported.

Backend wiring contract, quoted verbatim with attribution (Backend
Engineer, 2026-11-14; normative reference until the constructor
slice lands, which carries the code):

> SEALED WIRING HANDLE. `pub struct DispatchWiring {
> runtime_delivery: Arc<DeliveryContext>, catalog: BundleCatalog,
> peers: Arc<PeerConnectionManager> }` — all fields private, no
> struct-literal construction, no test-only constructors (tests use
> the mirrored path below like any embedded host, so no provisional
> bypass can become supported). Two constructors, the only ones, and
> neither accepts a caller-supplied pair: (1)
> `DispatchWiring::live(runtime: &Arc<RelayRuntime>, serve:
> &ConnectionServeContext)` clones the serve pair and stamps
> `serve.delivery_context().clone()` — the domain-claimed delivery,
> so the binding is the domain machinery itself, not a string; (2)
> `DispatchWiring::mirrored(runtime: &Arc<RelayRuntime>, peers:
> &[PeerConfiguration], autostart: bool)` builds the pair from that
> runtime's own roots (config-derived memberships, Hold/Run intent
> rule, `from_configuration`) and stamps
> `runtime.delivery_context().clone()` — inherently bound, nothing
> caller-supplied. Dispatch-time check inside experimental
> dispatch_send, before any other work:
> `Arc::ptr_eq(runtime.delivery_context(),
> &wiring.runtime_delivery)`, else refuse with
> `validation_wiring_runtime_mismatch`. No signature change needed
> (delivery Arc is already threaded to handle_send). Consequences,
> stated as guarantees: a mismatched runtime fails closed
> (different delivery Arc); an empty foreign catalog cannot be
> wrapped (no constructor takes bare catalogs); a same-root
> reconstructed pair binds its own fresh delivery Arc and fails
> closed against any other runtime, including a re-initialized
> successor of its own root — stale wiring is refused, never
> honored.

Reload semantics — option (b), precisely scoped (Backend
statement): catalog identity is shared-mutable
(`BundleCatalog` clones share one inner `Arc<RwLock<map>>`), so
watcher-driven bundle load/unload flows through every cloned handle
with no invalidation needed and no staleness possible. Peer-manager
identity is immutable per manager: a new peer set is a new manager
is a new wiring; old wirings keep their old-but-valid peer scope,
never a wrong scope. Exactly-once constructor discipline is
TRUSTED, not statically enforced: misuse degrades to fail-closed
refusal (brand mismatch) or fixed valid scope, never to
wrong-authority dispatch — that degradation property, not a call
counter, is the enforcement claim. Static-set construction
(`mirrored`) is valid where the hosted set is static for the
runtime lifetime; hosts that reload bundles at runtime must rebuild
the pair on the same events that rebuild the serve pair, or use
serve.

Unloaded-bundle refusal coverage rides with the Backend constructor
slice (non-member bundle target refused via the existing
`validation_unknown_bundle` path — pins no-probing and no
Run-defaulting). Intent rule the constructor applies, quoted for
citation: a bundle whose configured membership has autostart
disabled, or a relay started with autostart disabled, enters the
catalog as `HostingIntent::Hold`; otherwise `Run`.

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
2. ONE host-live shared pair: construction table above; no rebuild
   from files, no catalog copies, Arc/clone of live handles only.
   Mixture rejection is the sealed brand check (pointer equality on
   the domain-claimed delivery Arc), not path-string comparison;
   masquerade cases (mismatched runtime, empty foreign catalog,
   same-root reconstructed pair) fail closed by construction.
   Unloaded-bundle refusal test rides with the Backend constructor
   slice.
3. Principal per invocation; validate-against-owning-runtime after
   scheduling; spawn_blocking ownership; typed join failures
   distinguished from caller cancellation; truthful (no-recall)
   cancellation; heartbeat test with the operation.
4. Default-off experimental boundary, R1 cutover hold, runtime
   isolation, and the no-protocol-types rule (fields and
   constructor included) are preserved verbatim. Scope approval of
   this shape waives nothing and closes no S1/S2 implementation
   evidence.
