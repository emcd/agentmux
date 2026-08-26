# Relay Module

This directory contains relay internals and the public request/response types
exported from `src/relay/mod.rs`.

## Companion documents

- [`delivery-architecture.md`](delivery-architecture.md) — diagrams of the
  delivery path from `send` to terminal outcome, the relay/transport sequence,
  generation fencing, and the known gaps. Start here when reloading context on
  delivery.
- [`delivery-decisions.md`](delivery-decisions.md) — why the delivery contract has
  that shape: the retired timers, why no bound replaced them, why `Authorized`
  exists, and the tradeoffs behind each. Append-only.

## Primary Responsibilities

- Serve relay socket requests and stream-framed requests.
- Enforce authorization policy for list/send/look operations.
- Execute lifecycle transitions (`up`, `down`) per bundle.
- Route delivery across tmux and ACP transports.
- Maintain one unified session registry keyed by canonical `principal_id`
  (`session@namespace`), holding every known principal — bundle sessions,
  `users.toml`-declared relay-wide principals, and dynamic stream connections.

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
    by Hello verification and the `new peer` / `change psk` / `drop peer`
    tooling.
- `errors.rs`
  - relay error constructors and configuration error mapping.
- `client.rs`
  - relay socket client helpers and persistent stream session request/event
    polling.
- `catalog.rs`
  - the relay's live map of loaded bundles. Relay-wide rather than
    connection-scoped: the watcher writes it as bundles load, unload, and
    reload; the handlers and the host read it. Holds
    `CatalogEntry { paths, hosting_intent }` per loaded bundle:
    `HostingIntent::Run` is the default for an autostart bundle and is set by
    `up` regardless of current runtime state (the operator's request to host is
    the authoritative signal); `HostingIntent::Hold` is the initial intent for
    bundles without autostart and is what `down` sets when unhosting.
    `is_held()` is the single check the watcher uses to decide whether a
    configuration edit reloads or is suppressed.
- `connection/`
  - relay socket serving, split along the frame boundary; `mod.rs` is an
    import-only hub. `context.rs` holds the connection-independent handles a
    connection is served against. `serve.rs` owns one connection's lifetime —
    the registration drop-guard, the frame loop, and `ConnectionBinding`, the
    identity a Hello establishes for every frame after it. `framing.rs` sits
    below frame semantics: line reads off the socket half, and the
    blocking-pool hand-off that keeps synchronous handlers off the runtime's
    worker threads. `hello.rs` and `requests.rs` handle the two frame kinds,
    and `helpers.rs` holds the routing and error-shaping functions they share.
  - a handler lifted out of the frame loop cannot `break` or `continue` it, so
    both return a `FrameOutcome` and the loop performs the choice.
  - the Hello frame carries `principal_id` + `identity_token`; the token is
    verified against the principal store and the namespace decides binding.
    Session principals (`<session>@<bundle>`) look up their bundle in the
    `BundleCatalog` and bind the connection to it; non-session principals
    (`@GLOBAL`/`@EXTERNAL`/`@RELAY`) skip the catalog and are not bundle-bound.
    A request frame's optional `namespace` selects the routing bundle
    (overriding any binding); absent that, the bound bundle is used, and a
    relay-wide principal with neither is rejected.
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
- `configuration/`
  - the relay's own runtime configuration, distinct from `crate::configuration`,
    which resolves the configuration *roots* and the bundle/coder/policy
    artifacts within them. This module owns only what governs the relay process:
    whether it watches bundle files, whether it requires session credentials,
    its delivery quotas and timeouts, and the peers it dials. `mod.rs` is an
    import-only hub over `delivery.rs` (the `[delivery]` table, its ranges, and
    the cross-key quota relations), `relay_file.rs` (parsing `relay.toml`),
    `environment.rs` (the override layers and the precedence rule), and
    `runtime.rs` (the normalized `RelayRuntimeConfiguration` everything else
    reads).
  - `authorization` depends on this module, not the reverse:
    `load_authorization_context` reads one value from it, the choices queue
    bound it carries on `AuthorizationContext`. That bound is a relay setting
    rather than an authorization decision — no consumer of it authorizes
    anything — so it rides the context only because the context is what is
    already threaded from load down to the handlers.
- `authorization.rs`
  - policy loading plus the uniform, data-driven authorization stage
    (`authorize_route`): the requester's controls are always resolved in the
    dispatch (home) bundle and the maximum required tier across the route's
    targets is checked against the requester's configured scope for the
    operation's capability. Also exposes the discovery origin gate
    (`requester_list_reaches_all` / `authorize_discovery_origin`): the requester's
    `list` control must reach `all` before any cross-relay discovery lookup or
    peer dial.
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
  - relay-wide identity administration: `new peer` credential registration,
    `change psk` rotation, and `drop peer` deletion. Operates on the relay-level
    principal store with no bundle context; dispatched via
    `dispatch_identity_admin` before the per-bundle routing path in
    `connection/requests.rs`, which names each relay-wide admin request
    explicitly — a new one must be added there or it is routed as
    bundle-subject and fails with `validation_missing_routing_namespace`.
    Dropping deletes the store record and then invokes the same revocation
    helpers rotation uses; it refuses to drop the requester's own principal,
    since that would revoke the connection carrying the response.
- `handlers/discovery.rs`
  - relay-wide cross-relay discovery: configured relay-alias enumeration
    (`ListRelays`), and namespace/principal discovery (`DiscoverNamespaces` /
    `DiscoverPrincipals`) served locally or forwarded one hop to a configured
    peer. Dispatched via `dispatch_discovery` from `connection/requests.rs` alongside
    identity administration, with no bundle context. See Cross-Relay
    Discovery under Runtime Behavior Notes for the trust boundaries.
- `lifecycle.rs`
  - runtime reconcile/shutdown helpers for managed sessions, plus
    `preflight_bundle_configuration` — a read-only validation of a bundle's
    configuration through the same `load_bundle_configuration` +
    `load_authorization_context` path startup uses, with no tmux or runtime
    side effects (backs `agentmux check configuration`).
  - **A bundle's runtime is more than its tmux sessions.**
    `shutdown_bundle_runtime` is the single teardown every path reaches — `down`,
    a watcher unload, a watcher reload, and process exit — and it stops the
    delivery workers the bundle owns before touching tmux. It was tmux-only, so a
    worker (and, for an ACP member, its child agent) survived all four, and a
    reload handed the survivor straight back to the restarted bundle: the startup
    pass is idempotent for a registered worker, so an edited configuration
    reported success while the previous generation kept serving. Workers are
    identified the way their registry key identifies them, by namespace *and*
    runtime directory, so a stop cannot reach a same-named bundle hosted by
    another relay in the same process. A fail-stopped worker is skipped and stays
    registered — a bundle stopping is not evidence that an unobserved generation
    ceased, and the entry is what refuses a replacement.
  - **A per-session startup failure does not fail a bring-up.** Both
    `startup_loaded_bundle` and `reconcile_loaded_bundle` record the failing
    session's cause and carry on with the rest: a bundle is a set of sessions,
    and one failing is not a reason to withhold the others. Errors no single
    session owns — a tmux state query, a prune, principal registration, a
    panicked reconcile worker, a failed history write — still fail the whole
    operation, because a pass that cannot say what it did has nothing to report.
  - **A startup-failure record does not outlive the condition it describes.**
    `append_startup_failure` writes it; a session next observed serving —
    whether by a successful startup or by a successful delivery to it — calls
    `note_session_served_successfully`, which clears every record for that
    session. The history answers "is this session failing now", not "has it ever
    failed". Clearing applies on *every* such observation rather than the first:
    a session may fail more than once, so the in-memory cache that keeps the hot
    delivery path off the history file is a cache of *"this session's history is
    empty"* and is evicted by the next recorded failure. A cache that instead
    recorded having cleared once strands every failure after the first.
  - **The history is not an input to `startup_health`.** `list_bundle_state`
    derives health from readiness alone — healthy when every configured session
    is ready, degraded when at least one is and at least one is not, down when
    none are — and readiness is evaluated per list payload rather than carried
    forward from a startup pass. The `up`/`down` outcomes report that same
    readiness, not the log. What the history feeds is `startup_failure_count`
    and `recent_startup_failures`, which the TUI renders as its own lines. So a
    record outliving its session's recovery costs the diagnostic list its
    credibility — it reports a failure for a session that is serving — rather
    than lowering the health verdict, and the two must not be re-coupled by
    reading the log to decide readiness.

    Both rules are normative in the `bundle-lifecycle` capability, under
    `Bundle Startup Health Model` and `Startup Failure Visibility Contract`.
    This section explains them; those requirements govern.
  - Both passes bring up **every configured member** through the one
    `startup_member` helper, and report `ready_session_count` from its results
    rather than from what they created. Reconcile's tmux phase creates only tmux
    sessions, so without this an ACP member — which has no session to create —
    would be judged by observation alone and reported failed for the sole reason
    that `up` never tried to start it. `startup_member` is idempotent per
    transport, so a repeated pass over a healthy bundle changes nothing.
  - `session_ready_for_list` (in `handlers/listing.rs`) is deliberately **not**
    that helper — it answers "is this member serving right now", where
    `startup_member` answers "bring it up and tell me whether it is serving" —
    but it applies the same per-transport **condition**: `resolve_active_pane_target`
    for tmux, `acp_readiness_is_ready` for ACP. One session must not be ready on
    one surface and failed on the other, because `degraded` is defined as one
    state across `up` and `list`.
  - **`Busy` is ready.** An ACP worker serving a turn can serve, so it counts
    ready on both surfaces. The startup poll always accepted `Available | Busy`
    while the list projection accepted only `Available`, which reported a bundle
    whose ACP member was mid-turn as `degraded` — or as `down` if it was the only
    member — for the whole turn. Both now read `acp_readiness_is_ready`.
- `stream.rs`
  - hello-frame parser, the unified session registry, identity collision
    handling, and event writer routing. The registry is one
    `HashMap<principal_id, RegistryEntry>` keyed by canonical `principal_id`; an
    entry records the parsed identity, transport binding (`SessionType`, from
    which look/raww capabilities are derived at check time), a
    `RegistrationSource` (`Configured` for static bundle/`users.toml` principals,
    `Stream` for dynamic-only connections), and the dynamic stream state
    (writer/revoke/authenticated identity) while connected. **Offline is a state,
    not absence**: a `Configured` entry persists across (dis)connects so look/raww
    resolve its capability whether or not it is connected, and a Hello attaches
    dynamic state to the static shell (flipping it online). Worker readiness is
    *not* stored on the registry entry; it lives on the per-target
    `AsyncWorkerEntry` and is surfaced through the watch-channel map in
    `delivery/observability.rs` (see the `delivery/` block below), so the
    registry itself stays purely about presence, capability, and identity.
    `register_stream` distinguishes a **live** identity-claim conflict
    from a **stale** one before attaching: an existing entry whose writer is
    still open is a live owner and yields `IdentityClaimConflict`; an entry
    attached to a closed writer is a dead connection whose drop-guard has not
    run yet, and is reclaimed in place (its dynamic state cleared, a
    `relay.stream.stale_claim_reclaimed` inscription emitted with the prior
    `stream_id`) so the new client does not depend on `HELLO_CONFLICT_RETRY_TIMEOUT_MS`
    to take over. Hosts the shared session-eviction core (`evict_streams`): a
    selector matches entries, each connected one is torn down (dynamic state
    detached, typed error frame written, teardown signal fired), then removed or
    kept as a static shell per the eviction scope. `revoke_streams_for_identity`
    (matched by verified `authenticated_identity`, used by `change psk` and
    `drop peer`; keeps static shells) and `evict_streams_for_bundle` (matched by
    namespace, used by
    the bundle watcher; removes entries) are thin wrappers over it — there is no
    independent per-feature eviction path.
- `watcher.rs`
  - runtime bundle file watcher. Watches the bundles configuration directory
    (debounced ~200ms via `notify`) and reconciles the loaded `BundleCatalog`
    against the on-disk set on each change: new files load and start the bundle,
    disappeared files unload it (evicting sessions with `runtime_bundle_unloaded`),
    modified files are torn down and reloaded (evicting sessions with
    `runtime_bundle_reloaded`). Content fingerprints distinguish a real edit from
    filesystem noise. Runs on a dedicated thread (filesystem/tmux work is
    blocking); the host disables it when resolved `watch-bundles` is `false`
    (`relay.toml` key, `--no-watch` CLI override, or the
    `AGENTMUX_RELAY_WATCH_BUNDLES` environment override). A bundle whose entry has
    `HostingIntent::Hold` (no autostart, or held by a `down`) is **not** torn
    down or restarted by an edit: the new content fingerprint is absorbed and a
    `relay.bundle.reload_suppressed_held` inscription is emitted, so the
    operator's hold intent survives configuration edits until an explicit `up`
    sets intent back to `Run`. A bundle loaded by the watcher with no autostart
    (or with `--no-autostart`) is seeded with `Hold` and emits
    `relay.bundle.loaded_held` instead of starting sessions, so the operator
    brings it up on demand.
  - an event is only ever a trigger to look. Reconciliation re-scans every layer
    from disk and diffs by fingerprint, so nothing about the outcome depends on
    which event arrived — which is what makes it safe to reconcile on a timer as
    well. `run_bundle_watch_loop` therefore wakes on each debounced batch *and*
    at least once per `BUNDLE_WATCH_SWEEP_INTERVAL`, because notification is
    best-effort on every platform and one of its failure modes is silent:
    `notify`'s FSEvents backend renders a coalesced record as `[Create, Remove]`
    for one path, and `notify-debouncer-full` cancels that pair as a file that
    never existed, emitting nothing at all. Without the sweep a bundle file
    created and later removed on macOS can leave the relay serving a definition
    that is gone. The sweep makes a dropped trigger a bounded delay rather than a
    permanent miss.
  - access-only batches are dropped rather than reconciled, and deliberately do
    **not** restart the sweep timer. Reconciliation reads every watched `.toml`,
    so acting on its own reads would rescan once per debounce interval forever;
    letting those reads postpone the sweep would starve the backstop on exactly
    the busiest relay. This is why `notify-debouncer-mini` is not a candidate
    here despite the relay using none of `notify-debouncer-full`'s other
    intelligence: mini collapses every event to `Any`, leaving no way to tell a
    read from a write.
- `tmux.rs`
  - tmux/process adapters used by delivery and look paths.
- `delivery/`
  - transport-specific delivery decomposition:
  - `admission.rs`: the request-boundary admission gate and its quota ledger.
    Every accepted entry reserves envelope count and canonical payload bytes
    against a per-target and a relay-global limit, atomically across both, before
    `queued` is returned; the reservation is released at terminalization and
    nowhere else. Three refusals happen here rather than after queueing: an
    exhausted quota, an envelope whose canonical payload exceeds its transport's
    maximum handover dimensions, and a `Pubsub` target, which is refused
    synchronously so no work is authorized merely to discover the
    forward-declared stub. Also owns the shared "which transport will deliver
    this target" judgement (`resolve_target_session_type`,
    `target_is_relay_wide`), because admission is the first point that needs it
    and the delivery worker delegates to it rather than deciding again.
    Also owns undelivered-queue reporting, driven on an interval arm in the
    relay host's accept loop: a periodic aggregate (suppressed entirely when
    nothing is waiting, so an idle relay writes no recurring zero) and a
    first-crossing warning per target. The warning is deduplicated per target
    rather than per entry, because the condition an operator acts on is that a
    target is not draining; re-arming is structural, since a target's usage
    record — and with it the warned flag — is dropped when its last entry
    terminalizes. Neither emission resolves an entry, releases quota, or changes
    a scheduling position: this is the only duration-triggered mechanism left on
    the waiting side, and it is sound because elapsing produces a record and
    nothing else. Both the quota limits and the reporting intervals come from
    `relay.toml`'s `[delivery]` table, published once during relay startup before
    the listener binds; before that (in tests, and on any path that never hosts a
    relay) reads fall back to the same defaults a missing `relay.toml` resolves
    to.
  - `guard.rs`: the queue entry state model (`Pending`/`Authorized`/`Terminal`),
    the delivery identities (batch, attempt), the typed
    submission evidence, and the guard's single evidence order. The types live
    here but the state itself lives on the admission ledger's entries, under the
    lock that also releases quota — the terminal transition and the release are
    one atomic operation, and splitting them across two structures is exactly how
    a released reservation could end up on a still-live entry. `Pending` is
    unbounded by design and holds nothing but its own reservation; `Authorized`
    holds the target's ordering position, which is why the bound belongs on that
    side. Every lifecycle trigger — collector panic, closed channel, graceful
    shutdown — terminalizes through the same evidence order rather than choosing
    an outcome, so a member the relay can prove was never handed to a transport
    resolves `not_submitted` instead of being smeared into `submission_unknown`
    by whichever event happened to fire.
  - `fence.rs`: the five-step generation fence — cooperative stop request,
    bounded observation, non-blocking forced termination, second bounded
    observation, verdict. It answers only *has execution ceased?*, which is not
    the same question as *has this member resolved?*: a member may terminalize
    `submission_unknown` long before its generation is fenced, while replacement
    and the target's ordering barriers stay held until the verdict is positive.
    Steps 1 and 3 are kept distinct because step 3 is destructive — collapsing
    them would tear down a child that was about to stop on its own. Neither
    observation is a join: no runtime primitive can force a thread blocked in a
    syscall to return, so a join would reintroduce the unbounded wait the bound
    exists to close. Timeout and failure both route to a negative verdict, which
    is fail-stop by choice — a target that admits no new generation is
    operator-recoverable, and one whose old generation writes alongside a new one
    is not. The protocol is step-driven rather than awaited, because unit
    evidence stays admissible through both windows: a caller that awaited the
    fence as one future would stop collecting the very outcomes the fence exists
    to let it keep collecting. `acknowledge_fence` is a thin awaiting driver over
    the same state machine, so there is one implementation.
    Three things fence a generation, and two of them share a path: graceful
    shutdown and a stop of the worker's own bundle both end it through
    `stop_drain`, differing only in the `StopCause` they carry, while the
    execution watchdog fences from inside the worker loop. The cause is what the
    verdict is labelled with (`graceful_shutdown` or `bundle_stop`) and what an
    unresolved member's trigger reports, because a bundle stopping on a relay that
    keeps serving every other bundle is not a relay shutdown and must not be
    reported as one. The verdict is the resolution cut
    for both: still-unresolved members terminalize there through the guard's
    evidence order from *either* verdict, and only a positive one proceeds to the
    transport's own teardown — a negative verdict is the finding that those
    executors were never observed to stop, so reaping and joining them would run
    the bounded fence straight back into the unbounded wait it exists to replace.

    **Shutdown budgets nest, and the nesting is load-bearing.** Every bounded
    step on the shutdown path runs inside the watchdog grace that ends in a
    forced exit, so each sizes itself from what *remains* of one process-wide
    deadline rather than from a duration configured in isolation:

    ```
    watchdog forced exit           the hard limit: grace starts when the
    │                              watchdog OBSERVES the flag
    │
    shutdown-work deadline 5000ms  established at the FIRST of: watchdog
    │                              observation, or the first step needing a
    │                              budget after the signal. Never later than
    │                              the forced exit; usually a poll earlier
    ├── connection drain   1500ms
    └── cleanup_relay_host
        ├── async-delivery wait    min(1500ms, remaining − 1000ms reserve)
        │   └── generation fence   min(configured, remaining − 300ms) / 2
        │                          per window — the fence spends TWO
        └── socket + tmux teardown ← what the 1000ms reserve is for
    ```

    The two deadlines are deliberately not the same instant. A step that waited
    for the watchdog to publish before computing a budget would make every
    shutdown depend on that thread being scheduled promptly under exactly the
    load that makes shutdown slow; a step that assumed they coincided would hand
    out a deadline later than the exit it must precede. Arming early is the safe
    direction, and first-arming-wins keeps the earliest one.

    The deadline lives in `runtime::signals`; `budget_within_shutdown` is how a
    step takes its share. The rule exists because the durations have no
    relationship otherwise: `[delivery].fence-observation-timeout-ms` is
    operator-configurable to 60s and the fence spends two of those windows,
    nested inside bounds that neither know nor validate against it. Before this,
    raising a delivery timeout for an unrelated reason silently lost work on
    every shutdown — an admitted member could reach *no* terminal outcome at all
    when the process exited underneath the fence
    (`agentmux:issues/relay/65`). Two consequences worth keeping in mind when
    editing this path: a shutdown fence may be cut short and return a negative
    verdict, which here is a fail-safe rather than a transport fault, and
    members still queued to a worker resolve *before* the fence, because nothing
    about a never-authorized member's outcome depends on whether a generation
    ceased.

    The **execution watchdog** bounds a member's time from authorization to
    resolution by `[delivery].submission-timeout-ms`. It is a bound over the
    relay's own supervised code and nothing else, which it *is* only because
    every transport now resolves its outcome future at the write boundary rather
    than at the end of the turn. It states nothing about target health and
    produces no failure spelling: what it does on elapse is initiate the fence,
    and what resolves the members is the evidence order at the verdict.

    The two verdicts differ in what happens to the target afterwards. A
    **positive** verdict established that nothing from the old generation can
    still write, so the worker tears that generation down and builds a
    replacement in place, through the same `build_generation` its first one came
    from. A **negative** verdict could not
    establish it, so the target is marked fail-stopped: its registry entry is
    held for the rest of the process's life, which is what makes a replacement
    generation unelectable, and every further send — `mailw` and `raww` alike,
    since both reach the target through that one lookup — is refused with
    `delivery_target_fail_stopped`. Recovery is by operator action. That is the
    deliberate trade: a target that accepts nothing is recoverable, and a target
    two generations may be writing to concurrently is not.
  - `dispatch/mod.rs`: delivery dispatch re-export hub.
  - `dispatch/orchestration.rs`: delivery startup, ACP target priming, and the
    enqueue path that registers/feeds the per-target worker.
  - `dispatch/payload.rs`: structured delivery-message construction
    (`build_delivery_message`) and the out-of-band metadata inscription
    (`emit_envelope_metadata_inscription`), target-member resolution, and the
    prompt-batch settings read. Pane-envelope rendering, coalescing, and the
    token-budget combine now live inside each transport's internal delivery task,
    not here.
  - `dispatch/worker.rs`: per-target tokio worker task. A concurrent
    produce-and-collect loop (`select!` over `receiver.recv()` and a `JoinSet` of
    in-flight write outcomes) submits each task to its transport via the
    non-blocking `mailw`/`raww` seam — uniformly for every target, with no
    transport-type gate — and collects the resolved `OutcomeFuture`s. The
    blocking IO, quiescence/coalesce waits, ACP bootstrap/respawn, and readiness
    mirroring all live inside the transports now; the loop never names an ACP type.
  - `async_worker/`: split three ways along the boundary the module's own
    tests already followed. `registry.rs` holds the worker registry (tokio
    mpsc senders), the readiness/failure/output-view accessors, the ACP
    pending-slot accounting, and the shutdown drain helpers; `terminal.rs`
    holds the single terminal transition and the guard's evidence order
    (`complete_task_outcome` and the rest of the `complete_task_*` family);
    `reporting.rs` publishes the result. The dependency runs one way through
    the middle — `terminal` decides an outcome and hands it to `reporting`,
    which reaches back into `registry` only to route a receipt to a live
    worker. This single chokepoint records the `relay.log`
    observability floor for every terminal outcome and, for a *non-delivered*
    one (`Failed` incl. `pane_wedged`, `NotSubmitted`, `SubmissionUnknown`,
    `DroppedOnShutdown`),
    best-effort delivers a **terminal-outcome receipt** back to the original
    sender. The receipt is a relay/system-originated (`relay@RELAY`) envelope
    naming the original `message_id`, target, outcome, and any `reason_code`,
    routed through the sender's *own* transport by the normal delivery pipeline
    — so the sender learns of a non-delivery through its pane/turn rather than
    only from `relay.log`. It is built from the sender's home-bundle member and
    runtime directory (`AsyncDeliveryTask::sender_return_route`), never the
    target's, and routed with `try_existing_worker` so it reaches only an
    already-live sender worker and is dropped (not spawned, persisted, or
    retried) when the sender is not routable. `try_existing_worker` returns a
    typed `WorkerDispatch` (`Accepted` / `Missing` / `Closing`) so the enqueue
    path treats a worker draining for shutdown (`Closing`) as a drop rather than
    spawning a fresh worker that would clobber the closing registry entry the
    shutdown barrier still counts. A `delivered` outcome produces no
    receipt. Receipts are non-recursive: the `is_receipt` marker gates the spawn
    site so a receipt's own outcome never spawns another. A UI-class sender is
    unaffected — it still receives the `delivery_outcome` stream frame.
  - `choice_state.rs`: process-local choices queue (in-memory only; no persisted
    state) and the ACP chooser closure that captures `choices_pending_max`.
  - `observability.rs`: in-process pub/sub for the per-target worker-readiness
    surface and the choices-queue mutation stream, exposed to tests and
    embedders. Worker readiness is a transport-agnostic
    `WorkerReadinessState` (`Initializing` / `Available` / `Busy` /
    `Recovering` / `Unavailable`) keyed on `AsyncWorkerKey`
    (`(namespace, runtime_directory, target_session)`); observers subscribe via
    `subscribe_worker_readiness` (returns a `watch::Receiver<Option<WorkerReadinessState>>`,
    late subscribers get a `None` until the worker first publishes) and the
    transport-agnostic driver publishes via `publish_worker_readiness`. The
    choices-queue side exposes `choices_pending_max` and the
    `choose_authorized_ui_sessions` snapshot read used by the relay listing
    path.
  - ACP lifecycle and prompt flow live in `src/acp/` (see `crate::acp`); the
    `delivery/` module is no longer the home of ACP internals.
  - UI delivery is a first-class transport (`crate::transports::ui::UiTransport`),
    not a relay-internal special case: the worker resolves UI-routed targets to a
    `UiTransport` and delivers via `mailw`. The relay-side stream-broadcast
    touchpoints are injected as closures by `dispatch/worker.rs`
    (`build_ui_transport_services`), so the transport never imports `crate::relay`.
  - There is no quiescence module. Nothing here waits on a target: readiness is
    a level the relay reads before authorizing, and a member that cannot be
    handed over stays `Pending` rather than parking on a timer. See
    `dispatch/worker.rs`'s `decide_gate` for the one place that judgement is
    made.

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
  (`@GLOBAL`/`@EXTERNAL`/`@RELAY`) receive events from every bundle; like every
  other principal they are keyed in the unified registry by canonical
  `principal_id`.
- PSK files (`<state-root>/bundles/<b>/sessions/<s>/identity.psk` for sessions,
  `<state-root>/peers/<alias>.psk` for peers) and the principal store
  itself are written with mode 0600 (owner read/write only).
- **Bootstrap lockout warning**: `require-session-credentials` is a relay-level
  setting (a single socket serves every bundle, so per-bundle enforcement is not
  a real boundary), resolved by precedence — `--require-credentials` CLI override
  > `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS` environment override >
  `relay.toml` > default (disabled). When enabled, sessions without a provisioned
  PSK file are rejected at Hello. Operators must register at least one principal
  via `agentmux new peer <session_id>@<bundle>` (or run with the default) before
  flipping enforcement on, otherwise no client can connect to drive recovery.
- **Outbound peer relays**: `relay.toml` `[[peers]]` entries are active
  outbound-only endpoints — each carries the local `alias`, an absolute
  Unix-socket `address` (TCP host:port is future work), and the `connect-as`
  identity, and is validated at startup. A `Send`/`Raww` addressed with the
  bang-path `<session>@<bundle>!<alias>` is forwarded to the peer this relay
  locally calls `<alias>`. The `alias` is **not free-form**: it MUST be the
  identity *this* relay issued that peer via its own
  `new peer <alias>@RELAY`, checked at startup and by
  `agentmux check configuration` against the principal store. That binding is
  what lets a peer authenticating inbound be named by the principal just
  verified, with no lookup and no second name to keep in agreement. The check is
  unconditional, so a peer this relay only dials must be registered too: an
  absent record is indistinguishable from a mistyped or stale alias, and excusing
  absence would accept both. Only the record's existence and type are checked —
  the outbound PSK is *not* compared against the record's hash, because the two
  are credentials issued in opposite directions. The presented identity is
  **per-peer**: `connect-as`
  is the identity that peer issued this relay (via its own `new peer`), presented
  as `<connect-as>@RELAY` when dialing it — there is no single relay-wide
  identity, because the *receiver* determines it (two peers can issue different or
  colliding identities to this relay). `[[peers]]` is outbound-only and takes no
  `scope`: **inbound** cross-relay authorization is the scope this relay grants a
  connecting peer's principal via `new peer <id>@RELAY --scope`, enforced by the
  target-side ingress filter (deny-by-default). Raw peer PSKs stay owner-only at
  `<state-root>/peers/<alias>.psk`; the principal store holds only hashes.
- **Expiry pruning**: records with an RFC 3339 `expires_at` in the past (and,
  fail-closed, any with an unparseable `expires_at`) are pruned. The store is
  pruned-and-persisted once at relay startup and pruned before each
  `new peer` / `change psk` / `drop peer` mutation so the persisted file stays
  clean. A record with no `expires_at` never expires.
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
- A `drop peer` deletion removes the record outright and then runs the same
  revocation path: the store record is the only copy of the credential hash, so
  a record that no longer exists must not keep authenticating a live connection.
  It is the first caller of that revocation contract for which the principal
  ceases to exist — rotation reaches the same teardown, but leaves a principal
  behind that reconnects with the new credential. Dropping the principal the
  requester authenticated as is refused with `validation_self_drop_forbidden`
  ahead of the authorization gate, since revoking the connection that carries
  the response would leave the operator unable to distinguish a committed drop
  from a failed one; the store-backed `validation_unknown_principal` check stays
  *behind* the gate, where answering it for an unauthorized caller would
  disclose whether an arbitrary principal exists. Credential files are never
  deleted — once the record is gone the file authenticates nothing, and the
  relay cannot know where the operator distributed it — so the response reports
  the relay-owned canonical path for session principals only.
- Credential administration is relay-wide, not bundle-scoped. `new peer`
  (`RelayRequest::NewPeer`) generates a PSK and stores its SHA-256 hash;
  `change psk` (`RelayRequest::ChangePsk`) rotates an existing principal's hash
  in place; `drop peer` (`RelayRequest::DropPeer`) deletes the record and takes
  no destination, having no credential to route. `new peer` and `change psk`
  carry a `CredentialDestination` selector that routes the raw value to exactly
  one sink:
  - **Response** (default): return the raw PSK once in the response.
  - **Path** (`output_path` / `--output`): write to the caller-named absolute
    path — refusing symlinks via `O_NOFOLLOW`, requiring an existing parent,
    mode 0600 — and omit the PSK from the response
    (`validation_invalid_output_path` on a bad path).
  - **Config** (`write_to_config` / `--write-config`): write to the principal's
    relay-owned canonical credential path
    (`<state-root>/bundles/<b>/sessions/<s>/identity.psk`) and omit the PSK from
    the response. Config is derivable only for **session** principals, whose
    location the relay owns; relay/user/application principals are rejected with
    `validation_config_destination_unsupported`, and a `principal_id` whose
    components are not a valid session identity (the configured session-id
    grammar plus the canonical bundle-name grammar — dotted bundle names allowed,
    but the traversal-only `.`/`..` segments and separators rejected) is rejected
    with `validation_invalid_principal_id` before any path is
    derived. (A relay peer's `peers/<alias>.psk` is the
    *outbound* credential keyed by this relay's local alias, independent of the
    inbound `connect_as` a `new peer <id>@RELAY` registers — so it is
    deliberately not a Config target.)
  The destination is validated and staged (unique 0600 temp sibling, fsync)
  *before* the store is mutated, and — for `change psk` — the live-connection
  revocation fires only *after* the sink commits. A rejected or failed
  destination never mutates the store or revokes a connection. Both the store
  and credential writes publish via an atomic rename whose mode is enforced on
  the temp *before* the rename, so the rename is the single commit point with no
  fallible step after it; a post-commit rename failure rolls the store change
  back (and surfaces `internal_credential_rollback_failed` if the rollback write
  also fails). Identity-admin store transactions are serialized at relay scope,
  so concurrent `new peer` / `change psk` / `drop peer` calls cannot interleave
  store persists and credential renames. All three authorize the requester
  relay-wide: the caller's policy preset (resolved from a session member's
  `policy_id` or a `@GLOBAL` operator's TUI-config policy) must grant
  `new.peer` / `change.psk` / `drop.peer` at the `all` tier — bundle-relative
  `home` scope is insufficient, and application/relay principals are denied
  fail-closed. The three controls are distinct: neither `new.peer` nor
  `change.psk` confers deletion, so a policy file predating the `drop` control
  permits none until an operator adds it.

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
  `home` (via the unified registry's namespace filter) but needs `all` to enumerate
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
  preset. The **cross-relay** ingress filter now occupies that single seam: the
  shared authorization stage (`RouteAuthorization`) is the one place every target
  operation passes through, so a peer relay's forwarded `Send`/`Raww` is gated
  there — `RouteAuthorization::Ingress` deny-by-default against the peer
  principal's registered `scope`, distinct from the tier-based
  `RouteAuthorization::Policy` — rather than through N per-operation edits.
  Existence still sorts before it (`validation_unknown_target` before
  `authorization_forbidden`), since the spine validates targets in its prepare
  stage before authorization. The **intra-relay** target-side filter remains
  deliberately deferred; see `ideas/relay` (inter-relay target filtering) for that
  open design thread.
- **Cross-relay sender attribution (`on_behalf_of`)**: a forwarded `Send`/`Raww`
  carries the requester the *originating* relay admitted, as an advisory
  `on_behalf_of` origin subject, distinct from `authenticated_identity` (which at
  the receiver names the *forwarding* relay's own peer principal). The origin
  relay stamps `on_behalf_of` with the `principal_id` the requester was
  **admitted** under — verified against the principal store, or accepted as a
  socket-trust claim. **Attribution follows admission rather than re-deciding
  it.** `require-session-credentials` is where that decision is made and the only
  place it is made: a relay requiring credentials admits no unverified principal
  and so forwards only store-backed attributions, while a relay accepting
  socket-trust has already accepted that claim everywhere else — local delivery
  attributes from it, the stream registry is keyed on it, every local recipient
  sees it — so withholding it from a peer would withhold no capability and only
  make cross-relay attribution disagree with the relay's own delivery. What the
  setting does *not* decide is whether attribution happens at all; both modes
  carry it, and they differ in whether a credential backed the identity carried.
  `authenticated_identity` stays absent for an unverified requester regardless,
  which is what keeps the two fields separately sourced: one says who the sender
  was admitted as, the other whether a credential backed it. The receiving relay
  honors an inbound
  `on_behalf_of` **only from a peer-relay (ingress) requester** (a non-relay
  requester cannot self-assert it; the value is dropped), then carries it
  uninterpreted into the delivered `incoming_message` envelope alongside
  `authenticated_identity` and echoes it on the `Send` response. It is **advisory
  and single-hop**: never an authorization input (the ingress filter still gates
  solely on the peer's registered `scope`), not chained onward to a third relay,
  and read only relative to `authenticated_identity`. Raw input (`Raww`) has no
  delivered attribution envelope, so `on_behalf_of` rides the wire for symmetry
  but is not surfaced on delivery.
- **The delivered sender names both parties.** With an `on_behalf_of` present the
  delivered identity is `<origin>!<peer>` — the origin the peer asserted, and
  this relay's name for the peer that asserted it (its issued identity, per the
  alias invariant above). Both are needed: the relay authenticates the peer and
  never the foreign origin, so an identity carrying only the origin would give an
  advisory claim the authority of a verified sender. It is composed where the
  delivered message is built, not at pane rendering, because the pane header, the
  `incoming_message` event and the envelope metadata record all read one field —
  composing later would fix the pane and leave the event and the audit record
  naming the forwarding relay. Without an `on_behalf_of` the sender is the peer
  principal itself, qualified once.
- **The origin segment is copied, never inspected.** A peer may assert something
  that names no routable recipient; it is emitted unaltered, because the
  provenance is accurate whatever the shape and suppressing it would discard the
  only record of what was claimed. Such an identity displays but does not
  resolve: a reply fails at the replying relay's own target resolution. That is
  the reply guarantee's shape — it holds for the routable principal ids a
  conforming forwarding relay stamps, and it is conditioned on the peer rather
  than on a check here, since inspecting the claim to decide would be the
  interpretation the receiver is forbidden to perform. A reply can never reach
  the *wrong* peer, because the peer segment is derived locally.

### Cross-Relay Discovery

- Discovery answers the operator's "what can I address across relays?" question
  for cross-relay `Send`/`Raww`. It is **relay-wide**, dispatched at the
  connection layer (`dispatch_discovery`) like identity administration rather
  than through a bundle's `handle_request`, and covers three shapes:
  - `ListRelays` — enumerate this relay's configured outbound peer aliases
    (normalized `RelayRuntimeConfiguration.peers`), sorted and deduped. It reads
    configuration directly and **never dials** a peer, and never discloses
    address, `connect-as`, or credential detail — only the alias.
  - `DiscoverNamespaces` / `DiscoverPrincipals` — served locally when no
    `relay` selector is present, or forwarded a single hop when it is.
- **Two trust boundaries** gate foreign (forwarded) discovery, mirroring the
  cross-relay `Send` split:
  1. *Origin authorization.* Before any lookup or dial, the origin gate
     (`authorize_discovery_origin`) requires the requesting principal's `list`
     control to reach `all`; a narrower scope is rejected
     `authorization_forbidden` before the peer is contacted. The requester
     identity is the authenticated Hello principal (canonical
     `full_requester_principal_id`), never a wire field.
  2. *Receiving-relay ingress.* A forwarded request arrives with its `relay`
     selector cleared (no transitive re-forwarding) and **no** `on_behalf_of`,
     and the receiving relay derives every result from its own bundle catalog +
     `GLOBAL` registry, filtered by the authenticated peer principal's
     registered ingress `scope` via `scope_permits` — the same
     `RouteAuthorization::Ingress` deny-by-default authority that gates
     forwarded `Send`/`Raww`. An absent scope yields `authorization_forbidden`.
- **No existence disclosure across the boundary.** A namespace the peer's scope
  does not cover is omitted rather than reported as forbidden-because-present,
  and a concrete out-of-scope namespace returns `authorization_forbidden`
  without confirming the namespace exists. Foreign bundle/namespace ids derive
  solely from the receiving relay's catalog/registry and are never rewritten or
  injected by the origin request.
- **Partial marker.** When ingress scope narrows a bundle to an exact-principal
  subset (rather than a whole-namespace grant), the receiving relay stamps
  `principals_partial=Some(true)` on that `ListedBundle` via
  `build_scoped_namespace_bundle`; a complete listing leaves it `None`. The
  builder reuses the canonical `build_listed_bundle` (extracted in
  `handlers/listing.rs`) and then retains only scope-permitted principals, so
  readiness/state folding is not duplicated in discovery. A subset listing also
  **suppresses every bundle-level diagnostic** (`hosted`/`state`/`startup_health`
  and the startup-failure history/count): those describe namespace-wide state
  outside the grant and would otherwise leak out-of-scope session ids, reasons,
  and failure details. A subset view is addressing-only.
- **`GLOBAL` discovery.** `GLOBAL` is registry-backed, not a catalog bundle, so
  foreign `GLOBAL` principal discovery builds its listing from the unified
  registry (`list_namespace_sessions`) scope-filtered by the peer's ingress
  scope, rather than the bundle-configuration path — consistent with `GLOBAL`
  namespace discovery, which advertises it from the same registry.
- Discovery dispatch emits `relay.discovery.*` request / success / relay_error /
  unexpected_response / io_error inscriptions.

### Delivery

Diagrams and the decision history live in
[`delivery-architecture.md`](delivery-architecture.md) and
[`delivery-decisions.md`](delivery-decisions.md). The notes below are the
operational details that do not fit a diagram.

- Chat delivery is async-only; `delivery_mode` is no longer part of the relay
  send API. With the field removed, an internally tagged request silently
  ignores it like any other unrecognised field.
- The send API carries no caller-supplied delivery timeout, and no per-coder
  configuration bounds a delivery's wait on a target that is reachable but not
  ready — such a member stays `Pending` indefinitely. Per-target admission
  quota bounds the consequence, and the undelivered-queue inscriptions are how
  the backlog is observed.
- Reachability is the other axis and is bounded. A target continuously
  `Unreachable` for `[delivery].unreachable-dwell-ms` resolves its members
  `not_submitted` (`delivery_target_unreachable`) at the dispatch worker's
  health gate, so a dead target's queue drains rather than growing forever.
  Health says whether a handover is possible; readiness says when.
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
  from a `tokio::sync::mpsc::UnboundedReceiver`. The worker is a concurrent
  produce-and-collect loop: it submits each task to its transport via the
  non-blocking `mailw`/`raww` seam and collects the resolved `OutcomeFuture`s
  from a `JoinSet`, so a transport's blocking IO never pins the worker. Each
  transport owns its own internal delivery task and its `spawn_blocking` /
  blocking thread (tmux pane quiescence + paste; the ACP single-flight
  prompt-completion wait; ACP bootstrap and a driver-owned respawn monitor).
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
- Coalescing now lives inside each transport's internal delivery task, not the
  worker. The worker renders each task individually and submits it via
  `mailw`/`raww`; the transport buffers writes on its own ordered channel and,
  during its readiness/quiescence wait, absorbs contiguous envelopes into one
  flush group (tmux: one paste-buffer sequence against the resolved pane; ACP:
  one `session/prompt` turn respecting the prompt-token budget, with overflow
  left on the channel for the next turn). FIFO ordering at the target is
  preserved because the worker enqueues to the transport in receive order, and a
  raw write acts as a batch barrier that flushes the preceding envelope group
  first. The worker no longer batches, hoists the quiescence wait, holds a carry
  buffer, or emits a `batch_drain.coalesced` inscription.
- On shutdown the worker signals its transport(s) to resolve every in-flight
  write with `DroppedOnShutdown`, collects those resolutions, then drops any
  not-yet-submitted queued tasks (`complete_task_on_shutdown`). The transport
  contract guarantees prompt terminal resolution on shutdown, so the drain is
  bounded; the relay binary additionally tears its runtime down with
  `Runtime::shutdown_timeout` as a final guarantee.

### Connection lifecycle and shutdown

The connection-write and shutdown paths are bounded by mechanisms that
each came from a specific root-cause fix; each fix is now canonical
behavior rather than an incident.

- **Half-open zombie socket.** Each connection's writer task
  (`spawn_stream_writer`, `src/relay/stream/mod.rs:289-315`) wraps every
  `write_all + flush` in `relay_connection_write_timeout`
  (`src/relay/stream/mod.rs:323-330`). On timeout or write error the
  writer exits and drops its receiver, so further `send` calls surface
  `SendError`. **A bare writer break alone strands a half-open zombie**:
  `OwnedWriteHalf::drop` issues `shutdown(Write)` on the socket while
  the read loop remains parked on `read_line().await` with no
  post-hello read timeout, so the relay keeps reading from a
  half-closed peer and the peer's MSG_PEEK liveness check sees EOF
  (`0` = "stale"). The fix is the read-vs-writer race in
  `serve_connection` (`src/relay/connection/serve.rs:137-148`, a
  `tokio::select!` over the frame loop, the writer handle, and the
  per-connection revoke signal): a writer exit cancels the parked
  read loop, the `RegistrationGuard` drop below unregisters the
  stream, and the client observes a clean EOF on both halves and
  reconnects. Landed in `c963e5d`; covered by the stalled-client,
  idle-UI-flood, and `user@GLOBAL` cases in
  `tests/unit/relay_stream/robustness.rs`.
- **SIGTERM → 90s → SIGKILL shutdown hang.** Dropping the relay's
  `#[tokio::main]` runtime blocks until every `spawn_blocking` task
  finishes, and an unbounded `wait_for_prompt_complete()` pins its
  blocking thread forever. The current shape layers three bounds:
  (a) per-target ACP workers poll `shutdown_requested()` between
  receives and the single-flight prompt wait is bounded
  (`wait_for_prompt_complete(timeout)`, `src/acp/client.rs:443-458`),
  so the blocking thread is interruptible across shutdown; (b) the
  worker's shutdown drain
  (`src/relay/delivery/dispatch/worker.rs:289-301`) explicitly calls
  `transport.shutdown()` (`AcpStdioClient::shutdown()` at
  `src/acp/client.rs:473-484` kills/reaps the child); `Drop for
  AcpStdioClient` at `src/acp/client.rs:801-805` is the defensive
  backstop that calls the same `shutdown()` method if the transport is
  dropped without an explicit one; (c) the host's bounded
  `wait_for_async_delivery_shutdown`
  (`src/relay/delivery/async_worker/registry.rs`) bounds the join
  window, and the relay binary tears its runtime down with
  `Runtime::shutdown_timeout` (`src/bin/agentmux.rs:11,47`, with a
  5s `RELAY_HOST_SHUTDOWN_TIMEOUT` constant) as a final guarantee so
  any residual stuck blocking task is abandoned within a bounded
  window rather than hanging the process. Landed through the
  transport-abstraction + prime-timeout slices and locked in by the
  `fdec8a9` regression test (RG-approved), merged as `103b644`
  (`--no-ff`).
- **`EINVAL` from a socket option on a shut-down socket (macOS only).**
  POSIX allows `setsockopt` to fail with `EINVAL` once the socket has
  been shut down; Darwin implements that clause and Linux does not. So
  whenever the relay writes a frame and then closes — a revocation, a
  bundle eviction, a drain — the peer's *next* socket-option call fails
  on macOS while the frame it wanted sits readable in the receive
  buffer. Every such option here exists to bound a wait, and a shut-down
  socket cannot make a read wait, so the client treats the failure as
  ignorable and reads on (`is_ignorable_socket_option_error`,
  `src/relay/client.rs`), which is what recovers the frame. This
  presented three times before it was named — twice papered over at the
  classifiers (`add0ff4` for connect setup, `8e20927` for option resets)
  and once as a macOS-only unit-test failure — because the symptom
  appears at whichever call happens to follow the close, never at the
  close. The test helpers that arm a per-read timeout tolerate it for
  the same reason (`arm_frame_wait`, `tests/unit/relay_stream.rs`).

The first two share a theme: blocking/relayed work whose failure or
shutdown handling was incomplete — the connection write-timeout's
failure path was a half-open zombie (one half closed, the other
pinned), and the delivery runtime's failure path was a pinned
blocking thread on `Runtime::drop`. Both layers now collapse to
clean teardown — the connection EOFs both halves and the worker
returns + drops its child — so the runtime drop is not the single
commit point. Delivery is unbounded by design (see Delivery above),
so a stalled client fails that connection rather than throttling
the relay.
