# Relay Module

This directory contains relay internals and the public request/response types
exported from `src/relay/mod.rs`.

## Primary Responsibilities

- Serve relay socket requests and stream-framed requests.
- Enforce authorization policy for list/send/look operations.
- Execute lifecycle transitions (`up`, `down`) per bundle.
- Route delivery across tmux and ACP transports.
- Maintain stream endpoint registration keyed by `(bundle_name, session_id)`.

## File Map

- `mod.rs`
  - public re-export hub plus relay entrypoint wrappers.
- `contract.rs`
  - relay request/response enums and public payload structs.
- `context.rs`
  - internal request context and delivery task structs shared across relay
    submodules.
- `constants.rs`
  - relay-local constants shared across submodules.
- `identity.rs`
  - canonical/bare session identity helpers; principal store schema and
    load/persist primitives; PSK generation (`generate_psk`), SHA-256 hashing
    (`hash_token_sha256`), and `principal_id` namespace classification used
    by Hello verification and the `new peer` / `change psk` tooling.
- `errors.rs`
  - relay error constructors and configuration error mapping.
- `client.rs`
  - relay socket client helpers and persistent stream session request/event
    polling.
- `connection.rs`
  - relay socket serving, stream hello/request frame dispatch, Hello credential
    verification, and connection write-timeout handling. The Hello frame carries
    `principal_id` + `identity_token`; the token is verified against the
    principal store and the namespace decides binding. Session principals
    (`<session>@<bundle>`) look up their bundle in the `BundleCatalog` and bind
    the connection to it; non-session principals (`@GLOBAL`/`@EXTERNAL`/`@RELAY`)
    skip the catalog and are not bundle-bound. A request frame's optional
    `namespace` selects the routing bundle (overriding any binding); absent
    that, the bound bundle is used, and a relay-wide principal with neither is
    rejected.
- `drain.rs`
  - cooperative connection-worker shutdown. `ConnectionDrainCoordinator` is
    shared between the relay host and its connection workers: the host fires
    the shutdown signal and waits a bounded window for workers to drain, with
    per-worker state (parked vs mid-request) tracked through
    `ConnectionWorkerSlot` registrations so the drain report distinguishes
    drained, parked, and still-serving workers. Workers that miss the window
    are abandoned to runtime teardown and the shutdown watchdog.
- `routing.rs`
  - operation-agnostic routing/authorization spine. Defines the `OperationProfile`
    (which capability/control an operation reads) and the resolved-route types
    (`ResolvedRoute` / `ResolvedTarget`), and maps each target's relationship to
    the requester (self / same-bundle / peer bundle) onto a uniform scope tier
    (`self` / `home` / `all`). Consumed by the authorization stage.
- `authorization.rs`
  - policy loading plus the uniform, data-driven authorization stage
    (`authorize_route`): the requester's controls are always resolved in the
    dispatch (home) bundle and the maximum required tier across the route's
    targets is checked against the requester's configured scope for the
    operation's capability.
- `handlers.rs`
  - request dispatcher plus chat/look/raww handlers. `Send` and `Look` build a
    `ResolvedRoute` and authorize through the shared spine: a peer-bundle target
    raises the required tier to `all` while same-bundle access needs only
    `home` (self-inspection needs only `self`).
- `handlers/listing.rs`
  - bundle up/down and list-session request handlers. `handle_list_routed`
    separates the requester's home (dispatch) bundle — where its `list` control
    resolves — from the enumerated bundle, so a session may list a peer bundle
    under `all` without being looked up in the wrong bundle's members.
- `handlers/choices.rs`
  - choices snapshot, list, and pick request handlers.
- `handlers/identity.rs`
  - relay-wide identity administration: `new peer` credential registration and
    `change psk` rotation. Operates on the relay-level principal store with no
    bundle context; dispatched via `dispatch_identity_admin` before the
    per-bundle routing path in `connection.rs`.
- `lifecycle.rs`
  - runtime reconcile/shutdown helpers for managed sessions.
- `stream.rs`
  - hello-frame parser, stream registry, identity collision handling, and event
    writer routing. Hosts the shared session-eviction core (`evict_streams`):
    a selector matches live registry entries, each is evicted (entry nulled,
    typed error frame written, teardown signal fired). `revoke_streams_for_identity`
    (matched by verified `authenticated_identity`, used by `change psk`) and
    `evict_streams_for_bundle` (matched by bound bundle name, used by the bundle
    watcher) are thin wrappers over it — there is no independent per-feature
    eviction path.
- `watcher.rs`
  - runtime bundle file watcher. Watches the bundles configuration directory
    (debounced ~200ms via `notify`) and reconciles the loaded `BundleCatalog`
    against the on-disk set on each change: new files load and start the bundle,
    disappeared files unload it (evicting sessions with `runtime_bundle_unloaded`),
    modified files are torn down and reloaded (evicting sessions with
    `runtime_bundle_reloaded`). Content fingerprints distinguish a real edit from
    filesystem noise. Runs on a dedicated thread (filesystem/tmux work is
    blocking); the host disables it with `--no-watch`.
- `tmux.rs`
  - tmux/process adapters used by delivery and look paths.
- `delivery/`
  - transport-specific delivery decomposition:
  - `dispatch/mod.rs`: delivery dispatch re-export hub.
  - `dispatch/orchestration.rs`: delivery startup, enqueue, per-target
    orchestration, and the batch-drain entry point that fans one transport
    outcome out to N coalesced tasks.
  - `dispatch/payload.rs`: envelope/raw payload preparation, UI routing, and
    the multi-envelope batch preparer that packs coalesced envelopes against
    the prompt-token budget and peels the tail when ACP cannot accept the
    rendered multi-batch result in a single dispatch.
  - `dispatch/transport.rs`: ACP/tmux transport-specific dispatch, including
    the batch variants that share one quiescence wait (tmux) or one
    choice decision window (ACP) across the coalesced tasks.
  - `dispatch/worker.rs`: per-target tokio worker task; ACP bootstrap,
    respawn, and the blocking delivery body are run on the blocking pool
    via `spawn_blocking`. Holds a carry buffer that re-queues tasks the
    coalesce loop did not accept (mode-divergent head, ACP tail peeled to
    fit the single-batch invariant).
  - `async_worker.rs`: worker registry (tokio mpsc senders) and shutdown
    drain helpers.
  - `acp_delivery.rs`: ACP lifecycle and prompt flow. ACP session-id
    persistence and look-snapshot freshness derivation live in
    `crate::acp::state` (moved out of `delivery/` by decouple-transport-layer
    Slice 2a); the ACP client re-exports come directly from `crate::acp`.
  - `ui_delivery.rs`: UI-stream event emission for delivery completion.
  - `results.rs`, `quiescence.rs`: shared outcome and quiescence logic.

## Runtime Behavior Notes

### Identity and Credentials

- The principal store at `<state-root>/identity/principals.json` is the single
  authority for credential-to-`principal_id` mappings. PSK values are never
  persisted; only their SHA-256 hex digests are stored.
- Hello verification classifies the claimed `principal_id` by namespace, hashes
  the `identity_token`, and looks it up in the store with a constant-time
  comparison. A recognized token must be registered to the claimed `principal_id`
  (credential-to-identity binding). The `"socket-trust"` sentinel is accepted for
  session and user principals only when enforcement is off; application and relay
  principals always require a recognized credential. Relay-wide principals
  (`@GLOBAL`/`@EXTERNAL`/`@RELAY`) receive events from every bundle and are keyed
  in the stream registry by `principal_id` alone.
- PSK files (`<state-root>/bundles/<b>/sessions/<s>/identity.psk` for sessions,
  `<state-root>/peers/<peer_alias>.psk` for peers) and the principal store
  itself are written with mode 0600 (owner read/write only).
- **Bootstrap lockout warning**: `require_session_credentials` is a relay-level
  setting (a single socket serves every bundle, so per-bundle enforcement is not
  a real boundary), wired by the `--require-credentials` flag on
  `agentmux host relay` (default disabled). When enabled, sessions without a
  provisioned PSK file are rejected at Hello. Operators must register at least
  one principal via `agentmux new peer <session_id>@<bundle>` (or run with the
  default) before flipping enforcement on, otherwise no client can connect to
  drive recovery.
- **Expiry pruning**: records with an RFC 3339 `expires_at` in the past (and,
  fail-closed, any with an unparseable `expires_at`) are pruned. The store is
  pruned-and-persisted once at relay startup and pruned before each
  `new peer` / `change psk` mutation so the persisted file stays clean. A record
  with no `expires_at` never expires.
- **Expiry teardown at Hello**: the Hello path does not pre-prune the store;
  instead `verify_hello_credential` checks the matched record against the
  current time, and a recognized-but-expired credential is rejected with the
  distinct `runtime_identity_expired` error frame before the connection is
  closed (Slice 2). This tells an expiring session its credential lapsed rather
  than collapsing the case into the generic `validation_unrecognized_credential`
  it would see if the record had simply been pruned away. It mirrors the
  `change psk` revocation teardown below; both deliver a typed
  `runtime_identity_*` frame ahead of the close, distinct from the
  transport-level `relay_unavailable`.
- A `change psk` rotation replaces the stored hash immediately and revokes any
  live connection that authenticated with the old credential: the connection
  receives a `runtime_identity_revoked` error frame and is force-closed (Slice
  2). Revocation is keyed by the connection's verified `principal_id`, recorded
  on its stream-registry entry at Hello; socket-trust connections carry no
  verified identity and are never revoked. The registry entry is evicted before
  the teardown signal fires, so a reconnect presenting the rotated credential is
  not wedged into an identity-claim conflict against the dying connection.
- Credential administration is relay-wide, not bundle-scoped. `new peer`
  (`RelayRequest::NewPeer`) generates a PSK, stores its SHA-256 hash, and
  returns the raw value once — or writes it to an operator-supplied absolute
  path (refusing symlinks via `O_NOFOLLOW`, requiring an existing parent, mode
  0600). `change psk` (`RelayRequest::ChangePsk`) rotates an existing
  principal's hash in place. Both authorize the requester relay-wide: the
  caller's policy preset (resolved from a session member's `policy_id` or a
  `@GLOBAL` operator's TUI-config policy) must grant `new.peer` / `change.psk`
  at the `all` tier — bundle-relative `home` scope is insufficient,
  and application/relay principals are denied fail-closed.

### Cross-bundle routing and the uniform authorization model

- `Send`, `Look`, and `List` share one routing/authorization spine (`routing.rs`
  plus `authorize_route`). The invariant: the requester is always authorized in
  its home/dispatch bundle, and a peer bundle supplies only target existence and
  runtime/transport context — never the requester's policy controls.
- Authorization is **fully data-driven**: no operation carries a hardcoded
  cross-bundle policy in code. The spine classifies each resolved target's
  relationship to the requester (self / same-namespace / other namespace), maps
  it to a uniform scope tier (`self` / `home` / `all`), and checks the
  requester's *configured* scope for the operation's capability against the
  maximum tier the route demands. Whether a capability can ever reach the
  cross-namespace (`all`) tier is governed solely by the policy schema's
  per-capability allowed-scope set (`parse_policy_controls`).
- **Home is the principal's native namespace**, not whichever bundle a request
  routes through (`requester_home_namespace`): a session's home is its bundle; a
  relay-wide principal's home is its reserved namespace (`GLOBAL` / `EXTERNAL` /
  `RELAY`). There is no "global operator" exemption — `home` confers authority
  only within the principal's own namespace, so a `@GLOBAL` operator can act
  within `GLOBAL` under `home` but needs `all` to reach into any bundle.
- Concretely, for cross-namespace targets every operation requires `all`;
  same-namespace access of another principal requires `home`; self-access
  requires only `self`.
- **The `@GLOBAL`-target routing invariant.** A relay-wide (`@GLOBAL`) *target*
  is the one exception on the *target* axis, and it is a **routing invariant, not
  a policy control**: relay-wide principals are delivered through the session
  registry (keyed by `principal_id`) rather than by crossing into a peer bundle,
  so reaching one is not a cross-namespace act. Such a target classifies at the
  home tier in `ResolvedTarget::tier` and never raises the bar to `all`. This
  is what lets a bundle-bound agent message an `@GLOBAL` operator — and one
  relay-wide principal message another (relay-wide → relay-wide) — under
  `home` instead of forcing `all` for ordinary agent→operator
  messaging. The invariant is asymmetric with a relay-wide *requester* reaching
  *into* a bundle: there the bundle is not the requester's home namespace, so
  that direction does require `all` (a `@GLOBAL` operator listing or
  messaging a bundle is the privileged-operator-preset case above). The
  asymmetry — and a possible future `home+` scope that would fold the `GLOBAL`
  namespace into a principal's home tier — is documented in this section.
- `Send` cross-bundle delivery requires `all`. Earlier slices left it
  effectively permit-all (the old `authorize_send` used a `self` floor that any
  configured `send` scope cleared); the spine corrects this to match the
  long-standing `Relay Send Scope Control` spec. **This is a breaking change**
  for callers that relied on permit-all cross-bundle send under `home`.
- `Look` accepts a `session@<peer-bundle>` target and reads the peer bundle's
  session snapshot; the peer bundle is resolved from the target suffix against
  the live catalog and the capture runs in that bundle's runtime directory.
- `List` accepts a peer bundle via the wire `namespace` selector and enumerates
  that bundle's sessions; the requester's `list` control is resolved in its home
  namespace, so a session can list a peer bundle under `all` (this removed the
  prior defect where a cross-bundle list was rejected as an unknown sender
  because the requester was looked up in the enumerated bundle's members). A
  relay-wide principal's home is `GLOBAL`: it lists the `GLOBAL` namespace under
  `home` (via the relay-wide registry path) but needs `all` to enumerate
  any bundle. Its controls resolve from the enumerated bundle's authorization
  context (where the TUI-config controls are replicated), but its *home* for the
  tier check is `GLOBAL`, not that bundle.
- Unknown peers surface as `validation_unknown_bundle` and unknown peer sessions
  as `validation_unknown_target`. The non-stream entry point carries an empty
  catalog, so it stays confined to same-bundle operations. `Raww` routing mirrors
  `Send`'s suffix inference: a bound session routes within its bound bundle, and
  a relay-wide (`@GLOBAL`) principal derives the routing bundle from the target's
  `@<bundle>` suffix, so a `@GLOBAL` operator can raww into a bundle target
  (`issues/relay/24`). `Raww` authorizes through the same uniform route spine as
  `Send`/`Look` (`authorize_route` / `required_tier`): the requester's `raww`
  control resolves in its dispatch bundle, a same-bundle target stays at
  `home`, and a cross-namespace reach — including a `@GLOBAL` operator
  reaching into a bundle — requires `all`. The `raww` policy control accepts
  `all` (the shipped `operator` preset sets it, since a `@GLOBAL` operator's
  home namespace holds only relay-wide UI sessions, which reject raww — so every
  usable raww target is cross-namespace). A bound session reaching a peer bundle
  is the same cross-namespace case and likewise needs `all`; true
  bundle-A → bundle-B raww routing remains a separate effort (`todos/relay/76`).

#### Authorization model: origin-side capability, no target-side filter

- Cross-bundle authorization today is **origin-side only**: the requester's home
  policy decides whether it may reach across the bundle boundary (`all`).
  The target bundle has no say over who inspects or messages its sessions. This
  is a capability model — the grant travels with the principal — and within a
  single relay it is sufficient and non-redundant, because the relay mediates
  both ends inside one trust domain.
- A target-side filter (a bundle declaring "who may look/send/list into me") was
  considered and **deliberately deferred**. It is a different authority axis
  (target exposure, "who may touch me") that *composes* with the origin
  capability rather than replacing it, and it is not yet justified intra-relay:
  a second inbound-policy site can drift from or contradict the origin scope, and
  the relay already enforces ingress. Two related axes are kept distinct: *who
  decides* (origin vs target) and *how granular* (today `all` is a blanket
  cross-bundle grant; finer "may inspect bundle-b but not bundle-c" control, if
  needed, is cheaper expressed as scoped origin-side grants than as target ACLs).
- The forcing function for target-side filtering is **cross-relay**, where the
  trust boundary makes a target relay's ingress filter load-bearing (a target
  relay cannot assume the origin enforced anything; its sensible default is
  deny-by-default for foreign origins, the opposite of the intra-relay
  default-open stance). The intent is to design that filter first and only then
  decide whether to project the proven shape down onto intra-relay bundles —
  rather than mirroring it onto bundles for symmetry alone. Note that there is no
  "global/relay-principal exempt tier": a relay-wide principal's home is its own
  namespace (`GLOBAL` / `RELAY`), so it reaches bundles through the same uniform
  `all` threshold as anyone else, just configured on a privileged operator
  preset. The natural insertion point for a future target-side filter now exists
  as a single seam: the `authorize_route` stage is the one place every target
  operation passes through, so an ingress hook is a localized addition there
  rather than N per-operation edits. See `ideas/relay` (inter-relay target
  filtering) for the open design thread.

### Delivery

- Chat delivery is async-only; `delivery_mode` is no longer part of the relay
  send API. With the field removed, an internally tagged request silently
  ignores it like any other unrecognised field.
- Delivery is unbounded by design — bounded only by relay lifetime via
  shutdown — so the send API carries no caller-supplied delivery timeout.
- Pre-hello idle sockets are reaped in host connection workers to prevent
  starvation (`AGENTMUX_RELAY_PRE_HELLO_IDLE_TIMEOUT_MS` override).
- Each connection owns a per-connection writer task (mpsc + tokio write half)
  that applies a relay-to-client write timeout
  (`AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS` override, default 5s). A
  stalled client cannot pin the writer — or, via cloned writer senders, a
  delivery worker — indefinitely: a tripped write emits a
  `relay.connection.write_timeout` inscription and the writer task exits,
  closing the channel so cloned senders surface the disconnect.
- The stream registry entry is released via a drop guard so an async-cancelled
  connection cannot leak a registry entry and wedge the next same-identity
  reconnect into an identity-claim conflict.
- Host accept loops emit `relay.connection_pool.metrics` on each accept
  (`max_connections`/`active`/`rejected` counts) so saturation against the
  unified `AGENTMUX_RELAY_MAX_CONNECTIONS` cap is observable. Connections that
  exceed the cap receive a `runtime_connection_limit_reached` error.
- Stream events are correlated by `message_id` for send completion workflows.
- Per-target delivery workers run as tokio tasks (`tokio::spawn`) reading
  from a `tokio::sync::mpsc::UnboundedReceiver`. The blocking ACP / tmux
  delivery body, the ACP single-flight prompt-completion wait, and the
  ACP bootstrap and respawn paths are offloaded to `tokio::task::spawn_blocking`
  so a tokio runtime worker thread is never pinned during transport IO.
  Worker tasks normally run on the host's main runtime; sync callers that
  enqueue work without an ambient runtime (CLI helpers, unit tests) fall
  back to a process-wide multi-thread runtime created on demand. Worker
  shutdown is observed via `shutdown_requested()` polled between receives,
  the same signal the registry-empty drain in
  `wait_for_async_delivery_shutdown` waits on. The single-flight ACP
  prompt-completion wait also polls that gate: it is a bounded, resumable
  `wait_for_prompt_complete(timeout)` rather than an unbounded `recv()`, so an
  agent whose turn never completes cannot pin the worker's blocking thread
  across shutdown (which would block clean teardown until SIGKILL). On a
  shutdown abandon the worker returns and drops its ACP runtime, whose `Drop`
  kills the child. As a final guarantee the relay binary tears its runtime down
  with `Runtime::shutdown_timeout` instead of an implicit drop, so any residual
  stuck blocking task is abandoned within a bounded window rather than hanging
  the process.
- Each worker iteration coalesces a burst of pending envelope-mode tasks
  into one transport delivery. After the first task is received, the worker
  drains additional ready tasks via `try_recv` up to
  `AGENTMUX_RELAY_BATCH_DRAIN_MAX` (default 32), stopping at any task that
  changes payload mode or UI-routing. The non-coalescing task is pushed
  back onto a local carry buffer so it heads the next iteration without
  losing its place. For tmux envelope-mode heads the worker then hoists
  the per-target quiescence wait out of the transport: it runs
  `wait_for_quiescent_pane` on the blocking pool, and on success performs
  a second `try_recv` drain (same predicate, same drain-max budget) to
  absorb any tasks that landed in the channel while the pane wait was in
  flight. The rendered envelopes then share one paste-buffer sequence
  against the pre-resolved pane (the transport skips its own wait when a
  pre-resolved pane is supplied). ACP heads skip the hoist: the ACP single-
  flight prompt-completion wait runs *between* worker iterations, so the
  next iteration's blocking `recv` plus initial coalesce drain already
  absorb anything that queued during the previous turn. ACP-coalesced
  tasks share one `session/prompt` dispatch and one choice-decision
  window; ACP cannot accept multi-batch payloads, so when the packer
  produces more than one prompt batch the tail tasks are peeled back to
  the carry buffer to preserve the single-batch invariant. A mid-batch
  transport failure propagates the same outcome to every task in the batch
  (one delivery, one outcome by construction). When two or more tasks
  coalesce, the worker emits a `relay.chat.batch_drain.coalesced`
  inscription with the total `drained_count`, per-task `message_ids`, and
  the `pre_quiescence_count` / `post_quiescence_count` split so the
  operator can distinguish bursts already queued at batch assembly from
  bursts absorbed during the quiescence wait.
