# Design delta 001: struct-held operation context (PROPOSED)

Status: PROPOSED — pending operator pre-review authority and Tier-1
scope/text sign-off. No wiring proceeds on this text. On sign-off it
folds into `design.md` Public API Shape / Boundary Design (§1.4),
`tasks.md` 1.4 + 2.x notes, `proposal.md` boundary wording, and
`specs/tool-operations/spec.md`, and this file is removed.

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
(`Arc<RelayRuntime>`), the host-live shared bundle catalog
(`BundleCatalog`, cheap inner-Arc clone), and the host-live shared
peer-connection manager (`Arc<PeerConnectionManager>`), with one
async invoke method taking the relay-verified principal context and
the typed input, returning the typed output or the canonical error:

```rust
pub struct SendOperation { /* illustrative only */
    runtime: Arc<RelayRuntime>,
    catalog: BundleCatalog,
    peers: Arc<PeerConnectionManager>,
}

impl SendOperation {
    pub fn new(
        runtime: Arc<RelayRuntime>,
        catalog: BundleCatalog,
        peers: Arc<PeerConnectionManager>,
    ) -> Result<Self, OperationError>;

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

### Why a struct, not a function (forced shape)

The free-function form cannot source the host-live pair: the runtime
yields roots and delivery but not the hosted set; per-call
reconstruction reintroduces the Tier-1-named defects (default
`HostingIntent::Run` instead of the host's Hold, unshared peer
sessions, file-existence authority inference); runtime-owned holders
would fork the watcher-mutated live truth. The pair always arrives
from a host that already owns it; the struct is that arrival
recorded in the type. `spawn_blocking` ownership (S2) additionally
requires owned state the three-param function cannot thread.

### Operation-state handle vs host/server object (normative)

An operation-state handle is a capability bundle, not a server: it
owns no transport, resolves no association, generates no schemas,
serves no connections, and cannot act without a per-invocation
verified principal. The "no host or server object" rule targets
`McpServer`, `ToolRouter`, transports, and association-bearing
hosts — objects that authorize, frame, or present. The struct holds
none of those capabilities, so it is not a host/server object under
this rule. Passing the struct where a caller previously passed the
runtime handle does not reintroduce a server dependency.

### Construction (one live pair, per context)

The operation struct constructor takes `(runtime, catalog, peers)`
and refuses construction unless every `catalog.snapshot()` entry has
`entry.state_root == runtime.state_root()` and
`peers.state_root() == runtime.state_root()` (via the tiny accessor
in the Backend slice below); violation fails closed with
`validation_invalid_arguments` naming both roots. Type-level
prevention is impossible (all three are plain data); this fail-fast
construction check plus the once-per-runtime call discipline is the
rule. No catalog or peer manager is ever rebuilt from files or
copied as a snapshot inside the operation path.

- Adapter (post-3.1): clones the serve context it already holds —
  `serve_context.bundle_catalog().clone()` plus the peer manager
  via the Backend-provided getter below. Same staleness semantics
  as the socket path (watcher-driven).
- Embedded callers: the Backend once-per-runtime constructor slice
  (returns the live pair; see contract below).
- Tests: direct construction with the prepare-mirrored pair until
  that slice lands, then switch to it. Raw prepare-mirroring is
  provisional and cannot close unloaded-bundle, reload, or
  peer-lifecycle coverage — that refusal test rides with the
  Backend constructor slice.

Backend constructor/getter contract, quoted verbatim with
attribution (Backend Engineer, 2026-11-14; normative reference
until the constructor slice lands, which carries the code):

> 1. ONCE-PER-RUNTIME CONSTRUCTOR. Signature:
> `impl RelayRuntime { pub fn shared_dispatch_wiring(&self,
> peers: &[PeerConfiguration], autostart: bool) ->
> Result<(BundleCatalog, Arc<PeerConnectionManager>), RelayError> }`.
> Attached to RelayRuntime (it owns configuration_roots and
> state_root); no new holder — it builds and returns, the host
> holds. Caller and timing: the embedded host (no serve context)
> calls it exactly once per runtime lifetime after
> `RelayRuntime::init` and shares the returned Arcs across all
> operations (clone per use; both types are cheap Arc clones).
> Serve-based hosts never call it — they clone
> `serve_context.bundle_catalog()` and the serve peer manager
> instead. Sourcing, no probing: memberships come from
> `configuration::load_bundle_group_memberships(&self.configuration_roots)`
> (config-derived, no startup side effects, no bundle startup);
> each member resolves via
> `BundleRuntimePaths::resolve(&self.state_root, name)`; the catalog
> is built with `from_entries`; the peer manager with
> `PeerConnectionManager::from_configuration(&self.state_root,
> peers)`. Static-lifetime caveat, stated: the returned pair is
> config-derived, not watcher-mutated — valid where the hosted set
> is static for the runtime lifetime; hosts that reload bundles at
> runtime must rebuild the pair on the same events that rebuild the
> serve pair, or use serve.
>
> 2. PEER-MANAGER GETTER. On ConnectionServeContext (new, additive,
> no behavior change): `#[must_use] pub fn peer_connection_manager(&self)
> -> &Arc<PeerConnectionManager>`. Mirrors the existing
> `bundle_catalog()` getter; needed so adapter-side construction can
> clone the live manager instead of rebuilding an empty one.
>
> 3. MIXTURE-REJECTION RULE. [Covered by the constructor check above;
> violation error named per this delta.]
>
> 4. Intent rule the constructor applies, quoted for citation: "a
> bundle whose configured membership has autostart disabled, or a
> relay started with autostart disabled, enters the catalog as
> `HostingIntent::Hold`; otherwise `Run`."

### Per-invocation principal, validated after scheduling

The struct holds no principal, grant, or authorization outcome. Every
invoke takes a fresh `VerifiedPrincipal`, and verification against
the owning runtime (`check_context`: binding epoch, instance
binding, shutdown gate) runs inside the dispatched work — after
scheduling, never cached from a prior call. `spawn_blocking` owns
cloned context, principal, and input; no lock is held across an
await (the struct holds no locks; catalog reads are short scoped
guards inside the blocking dispatch). A panicked or detached
dispatch task surfaces as `internal_unexpected_failure`, never as a
fabricated outcome. Cancellation is truthful: dropping the caller
future does not recall an accepted send — there is no recall
mechanism, and none is implied. The heartbeat test proves executor
progress while forwarding blocks on an unresponsive peer.

## Reconciliation notes (reviewer conditions mapped)

1. Complete contract amended: Public API Shape bullet replaced (see
   above); tasks 1.4 reworded to struct construction + invoke;
   spec Requirement 1 reworded to operation objects with the
   handle/server distinction stated normatively; proposal boundary
   wording clarified as above. Waves, BundleScope, error and
   validation rules untouched.
2. ONE host-live shared pair: construction table above; no rebuild
   from files, no catalog copies, Arc/clone of live handles only.
   Mixture rejection enforced at the operation constructor;
   unloaded-bundle refusal test rides with the Backend constructor
   slice (existing `validation_unknown_bundle` path).
3. Principal per invocation; validate-against-owning-runtime after
   scheduling; spawn_blocking ownership; typed join failures;
   truthful cancellation; heartbeat test with the operation.
4. Default-off experimental boundary, R1 cutover hold, runtime
   isolation, and the no-protocol-types rule (fields and
   constructor included) are preserved verbatim. Scope approval of
   this shape waives nothing and closes no S1/S2 implementation
   evidence.
