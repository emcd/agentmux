## Why

Three separately-named defects are one defect wearing three hats: **a transport
inferring delivery failure from the absence of change, and reporting it as
non-delivery for bytes it may already have committed.**

- **Wedge detection** infers failure from an unchanged screen. On Pty it reaches
  that verdict in under a second (`WEDGE_CONSECUTIVE_TICKS` 3 x `QUIET_WINDOW`
  50 ms), so an agent sitting on a tool-permission dialog fails its deliveries
  almost immediately.
- **Prime timeout** infers failure from absence of output. Same inference,
  different sensor. A pane awaiting a keystroke is silent and perfectly healthy.
- **Readiness timeout** infers "gave up" from a prompt that never returned.
  Honest on Tmux, where nothing was injected, but still us abandoning a healthy
  long-horizon agent turn while phrasing it as a fact about the pane.

The live `Delivery Classifier` requirement already states the rule these violate:
*"only a positively observed terminal event — process death, a closed connection,
a protocol error — is sound evidence of failure, and an unchanged screen is not."*
It sits four paragraphs above a timer that fires on absence.

Underneath all three is an **implicit commitment boundary**. Pty writes every
envelope to the pty master before its quiescence wait
(`src/pty/delivery.rs:307`, `:356-369`), so its prompt-readiness template gates
nothing and every non-delivered outcome it reports is a false statement. ACP is
the same shape — `client.prompt(...)` dispatches at `src/acp/transport.rs:1189`,
the prime timer starts at `:1199`, and the code comments acknowledge the prompt
may still resolve while reporting `Timeout`. Only Tmux commits after its wait.

The cost is not theoretical. **`agentmux:issues/relay/62`**: envelopes coalesced
into a Pty flush group *during* the wait are pushed onto the group
(`src/pty/delivery.rs:174`) but never written, while `send_group_outcomes`
(`:847`) resolves every member identically. Their senders are told the group's
outcome — `Delivered` on the normal path — for bytes that never left the relay.
A red regression test pinned this originally and was deleted along with the
transport-owned wait it exercised, so the defect currently has no test proving it
fixed; reinstating one against the relay-owned queue is the acceptance criterion.
Wedge detection has been *masking* it by cutting waits short, so retiring the
timers widens the window; the fix and the retirement must land together.

## What Changes

- **Establish an explicit delivery commit contract** that separates four events
  currently collapsed into one: **admission** (accept into the queue, reserve
  capacity, return `queued`), **authorization** (a relay-local `Pending` →
  `Authorized` transition — the single linearization point, after which the relay
  never reclaims), **submission** (a packing unit produces a target-side effect),
  and **resolution** (each member resolves once, from recorded facts). The relay
  owns the queue and the patience policy; transports observe and report
  positively.
- **Distinguish the batch from the packing unit.** A batch is the unit of
  *authorization*; a packing unit is the unit of *target submission*, and a batch
  is not one atomic target write. Tmux already splits a group into token-budget
  prompts injected separately (`src/tmux/transport.rs:659-697`), and ACP and Pty
  do the analogous thing. The partition is fixed and exact before the first
  side-effect, order is preserved, and **outcomes are per unit** — which Tmux
  already implements and Pty does not.
- **Move queueing relay-side; leave rendering and packing in the transport.**
  Transports declare capacity in units the relay can evaluate without packing —
  envelope count and canonical payload bytes. `prompt_tokens_max` stays an
  internal packing-unit limit, invisible to the relay.
- **One commit point for every transport.** Authorization is the linearization
  point on all of them; no transport decides when commit occurs. Transport-specific
  timing governs when readiness makes authorization *useful* and when submission
  evidence appears — it does not move commit or extend cancellation past it.
- **Invocation after authorization is fallible, and that is a terminal evidence
  result rather than a reclaim.** `mailw` can legitimately fail today — Tmux
  resolves `channel_full` when its write channel is full or closed
  (`src/tmux/transport.rs:159-178`) — and the relay's admission quota reserves
  nothing about a transport's channel, worker generation, or target resources. A
  refused
  invocation resolves the authorized unit as positively **not submitted** when
  the item comes back unchanged, or **submission unknown** when side effects
  cannot be excluded. The relay never reclaims either way.
- **Apply the contract to every transport, not three.** `TransportImpl` is
  `Tmux`, `Acp`, `Pty`, `Ui`, and the `Pubsub` stub. UI is first-class, always
  reports ready, and today uses a bounded reconnect wait
  (`src/transports/ui.rs:152-188`) — an absence timeout of exactly the kind this
  change retires. It is in scope for relay-owned queueing, readiness, capacity,
  and authorization.
- **BREAKING — retire every absence-inference timer.** Delete
  `[coders.<id>.pty].wedge-detection`, `[coders.<id>.{tmux,acp,pty}].prime-timeout-ms`,
  and `[coders.<id>.tmux].readiness-timeout-ms`, along with the `wedged`
  classification, `DeliveryWaitError::Wedged`, and the prime/readiness deadline
  machinery. No transport classifies a delivery from what a pane displays or
  from how long it has been quiet.
- **BREAKING — add no bound in their place.** An earlier draft replaced them with
  a relay-level residency bound resolving `expired`. That was the same inference
  relocated: elapsed waiting decided an outcome, and because expiry terminalizes
  and releases quota it dropped mail that would have landed once a long agent turn
  finished. A `Pending` entry whose target is reachable now waits indefinitely, and
  the `expired` outcome is deleted along with the timers. Sustained
  unreachability is bounded separately by `[delivery].unreachable-dwell-ms`,
  which qualifies a repeated observation rather than inferring from absence. What replaces them is a **relay-level admission
  quota and scheduling policy** — enforced positively at send time, per target and
  relay-global — plus **undelivered-queue inscriptions** that report a long wait
  without adjudicating it.
- **Fix `agentmux:issues/relay/62` structurally.** With the relay owning the
  queue there is no transport-internal queue for a message to be absorbed into
  after a write, so the defect becomes unreachable rather than patched. A
  reinstated coalesce-during-wait regression test, driving the relay queue rather
  than Pty's, is the acceptance criterion.
- **Make outcomes report evidence rather than position.** `delivered` requires
  transport-specific *positive* evidence of injection — `inject_literal_text`
  returning `Ok`, `write_all` to the master succeeding, a prompt accepted — and a
  readiness observation is not delivery evidence on any transport that writes
  after observing. Positive evidence that a unit produced no side effect soundly
  asserts non-delivery (`not_submitted`); absence of evidence yields
  `submission_unknown`. Submission success terminalizes `delivered` immediately,
  so a later exit or close is **target-health observability, not a delivery
  outcome** — there is no `target failed` resolution.
  `agentmux:issues/relay/61` closes here.
- **State the guarantee as at most one relay-authorized injection attempt**, not
  at-most-once delivery. Transports do not deduplicate attempt IDs, so the
  stronger claim would be false. Every accepted member resolves **at most** once
  in a surviving relay process, including when a transport panics. Uniqueness is
  guaranteed; completeness is not, because a member queued for a target that never
  becomes ready is never resolved by anything. Restoring completeness is the
  `fetch`-cursor work in `agentmux:todos/runtime/23`, not a timer.

### What does not change

Async undeliverable notification is not eroded — only its false-positive sources
are. Every receipt that fires today for a real reason keeps firing:

| Trigger | Kind | Status |
|---|---|---|
| Transport disappears while members are still pending | relay policy | stays; a transient disappearance need not drop |
| Relay shutdown, members still pending | pre-commit | stays (`dropped_on_shutdown`, pending members only) |
| Admission quota exceeded | our own policy, positively accounted | becomes a **synchronous rejection at send time**, with no receipt because nothing was accepted |
| Target exits after submission | positively observed | recorded as target-health observability, **not** a delivery outcome |
| Settled non-prompt frame | inference from absence | **retired** |
| No output within a window | inference from absence | **retired** |
| Prompt never returned within a window | inference from absence | **retired** |
| Relay queue residency exceeded | inference from absence | **retired**; the entry keeps waiting and the condition is reported by inscription |

**`raww` keeps one ordering and gains no modes.** Raw keeps today's FIFO
batch-barrier ordering — a raw item flushes buffered mail first, then delivers as
its own write (`src/transports/contract.rs:120-124`) — and waits for target-side
ordering safety of older mail, which requires that execution has ceased rather
than merely that an outcome has become terminal.

An operator emergency mode that overtakes `Pending` mail and bypasses readiness
gating was specified here and has been **removed from this change's scope
entirely**, on the operator's 2026-08-10 call. It is not deferred to this
change's Phase 2; it is filed as a standalone future item at
`agentmux:todos/transports/8`, where it is scoped to Tmux and Pty only. ACP is
structurally excluded rather than merely unimplemented: ACP raw is another
`session/prompt` and the client enforces one active prompt, so overtaking during
an active turn yields a serialization refusal rather than steering, and ACP has
no byte-stream primitive to type past with. Its recovery path is
choose/cancel/teardown.

**One behavior change to state rather than discover:** Pty raw today interrupts
an in-flight envelope wait (`src/pty/delivery.rs:634-665`). That interruption is
removed by this change and **nothing replaces it in this scope**. An earlier
draft preserved the capability by substitution, with relay-side emergency
overtaking standing in for the in-transport interrupt; with emergency raw out of
scope, the capability is lost until `agentmux:todos/transports/8` lands. This is
an accepted regression rather than an oversight — the operator confirmed the
capability is unused today.

**Crash recovery is out of scope.** 0.9.0 guarantees hold for a surviving relay
process and graceful shutdown. Seam constraints that keep durability reachable —
explicit `Pending`/`Authorized`/`Terminal` entry states and a stable attempt ID
per authorization — are required now because retrofitting them is expensive.
**In-process** recovery is specified: across a transport teardown and respawn,
`Pending` entries reschedule to the new generation and `Authorized` entries are
never re-invoked, resolving through the guard's evidence order before the
replacement begins. Respawn is a trigger for resolution, not a chooser of
outcomes.
**Process-startup** recovery is not specified, because nothing persists across a
process boundary in 0.9.0. Durable recovery is a follow-up issue.

### Phased implementation

One OpenSpec change, two implementation phases, per the operator's 2026-08-01
scope call. The spec deltas describe the whole contract; `tasks.md` draws the
line and Coordinator confirms it before implementation starts.

**0.9.0 — the core.** Relay-owned queue; admission, authorization, submission and
resolution; admission-quota and scheduling policy; retirement of wedge, prime and
readiness timers with no bound put in their place;
relay/62's structural fix; **and the minimum authorization guard and generation
fence** — guard creation atomic with authorization, terminal CAS with
exactly-once quota release, supervision of invocation/worker/collector/executor
exits, transport-generation identity, and enough fence authority that a
replacement cannot start and raw cannot pass until old execution ceased.

**0.9.x follow-on.** UI transport conversion; the guard surface beyond that
minimum.

**The governing invariant, and why the guard cannot be deferred:** *no transition
to `Authorized` may occur unless an owner capable of terminalizing and releasing
it is created in the same atomic operation.* An earlier draft of this proposal
deferred the guard on the grounds that exactly-once resolution is already not
guaranteed today. That was wrong on a dimension it did not check: this change
introduces per-target and relay-global quota in count and bytes which **releases
only at the terminal transition**, so an unowned `Authorized` entry does not
merely lose a receipt — it **leaks quota permanently and blocks the target
FIFO**, since it can neither expire nor retry. Today's collector panic at least
releases its pending slot
(`src/relay/delivery/dispatch/outcomes.rs:80-96`). Deferring the guard would
therefore *regress* resource accounting rather than hold it constant.

If the minimum guard cannot fit the core, the correct response is to defer the
irreversible `Authorized` model **and** the timer retirement together, not to
ship an unowned state — the timers are what currently bound a stuck delivery.

**Interim exception, stated as an implementation-phase exception rather than as a
property of the specified contract.** The deltas describe the end state; this is
where the 0.9.0 core knowingly does not yet reach it:

- **UI keeps its reconnect timer** until conversion lands, so timer retirement is
  not yet universal. The specs describe universal retirement as the contract; the
  core phase carries a named exception for `TransportImpl::Ui`.

Pty's raw-interrupt capability is **not** an interim exception either — it is
simply removed. An earlier draft preserved it by substitution, with relay-side
emergency overtaking standing in for the in-transport interrupt. With emergency
raw out of scope the substitution is gone and the capability lapses until
`agentmux:todos/transports/8` lands. Recorded as an accepted regression, since
the operator confirmed the capability is unused.

## Capabilities

### New Capabilities

None. The queue substrate lands inside `delivery-quiescence`, which already owns
async queue lifecycle and terminal-outcome semantics.

### Modified Capabilities

34 deltas across six capabilities. The `specs` artifact is authoritative; this
inventory is reconciled against it.

- `transport-abstraction` (7 MODIFIED, 3 ADDED, 4 REMOVED) — the commit
  contract's home. MODIFIED: `Transport Interface Contract` (batch seam, fallible
  invocation, no waiting), `Transport Module Boundaries` (separates queueing,
  readiness scheduling, and packing), `Synchronous Delivery Completion` (per-unit
  outcomes), `Worker Readiness Interface`, `Positive Activity Signal` (relay
  consumes it; no classifier), `Transport-Internal Probe Seam for Testability`,
  `Pty Transport Implementation` (buffer then write; singleton units). ADDED:
  `Packing Units and Typed Submission Evidence`, `Transport Generation Fencing
  and Termination Authority`, `Transport Handover Capacity and Readiness`.
  REMOVED: `Three-State Delivery Classifier`, `Prime Timeout Envelope Field`,
  `ACP Prime Timeout Envelope Field Consumption`, `Generalized Wedge/Prime State
  Machine`.
- `transport-contracts` (3 MODIFIED, 4 REMOVED) — MODIFIED:
  `Prompt-Readiness Template Gating` (gates authorization uniformly; no
  per-transport failure inference), `Relay raww transport behavior` (Pty and UI
  arms, and raw's wait on target-side ordering safety rather than terminality),
  `ACP Transport Error Code` (separates the synchronous code from the delivery
  outcome, and bounds the outcome to positively observed terminal lifecycle).
  REMOVED: `ACP Prime Timeout`, `Tmux Prime Timeout`, `Pty Prime Timeout`, `Pty
  Wedged State Detection`.
- `delivery-quiescence` (7 MODIFIED, 2 ADDED) — MODIFIED:
  `Quiescence-Gated Delivery` (relay-owned readiness), `Delivery Results Without
  ACK Protocol` (admission reservation, Pubsub rejection), `Async Queue Lifecycle
  and Ordering` (entry states, elapsed-time resolution only for sustained
  unreachability, per-target FIFO as worker-enqueue linearization, no
  cross-target scheduling),
  `Asynchronous Terminal-Outcome
  Receipt` (new outcome vocabulary), `Async Delivery Observability`, `Async Queue
  Growth Risk Disclosure`, `Quiescence Documentation`. ADDED: `Delivery
  Authorization and Terminal Guard`, `In-Process Delivery Recovery Scope`.
- `addressing-routing` (1 MODIFIED) — `Bundle Membership Configuration`, for the
  five removed descriptor keys.
- `runtime-bootstrap` (1 MODIFIED) — `Relay Configuration File` gains the
  `[delivery]` table that replaces them.
- `cli-surface` (1 MODIFIED) — `Send Timeout Override Flags by Transport` (its
  key enumeration is now relay-level).

**Deliberately not modified**, recorded so they read as decisions rather than
omissions:

- `transport-contracts` / `Transport Capability Contract` — it derives static
  look/write capability from `SessionType`. Maximum handover dimensions and
  `is_ready_for_handover` are a different axis (dynamic delivery capacity), and
  adding a fifth capability there would conflate them. They live in
  `transport-abstraction` / `Transport Handover Capacity and Readiness`.
- `transport-contracts` / `ACP Persistent Worker Lifecycle` — ACP's discarded
  `JoinHandle` is real and blocks fence acknowledgment, but the generic
  `Transport Generation Fencing and Termination Authority` requirement is
  normative for all five transports and already forbids it. The ACP-specific work
  is carried in `tasks.md` and tracked at `agentmux:todos/relay/128` rather than
  duplicated as spec text.
- `tui-surface` / `TUI raww *` — raw carries no mode discriminator, so the TUI's
  raww surface is unchanged by this change.

`session-relay/spec.md` carries hub *prose* that goes stale (a requirement total,
a partition description advertising "prime/wedge timeouts"). Not requirement
text, so it takes no delta; reconciled as a sync/archive task.

## Impact

- **Configuration — BREAKING, five keys deleted.** Reconciled against the
  authoritative descriptor list in `addressing-routing` / `Bundle Membership
  Configuration`, not assembled from memory:
  `[coders.<id>.tmux].prime-timeout-ms`,
  `[coders.<id>.tmux].readiness-timeout-ms`,
  `[coders.<id>.acp].prime-timeout-ms`,
  `[coders.<id>.pty].prime-timeout-ms`, and
  `[coders.<id>.pty].wedge-detection`. Any live `coders.toml` setting them fails
  to load on existing unknown-field validation. UI adds nothing to this list: its
  reconnect timeout is a constant plus builder (`src/transports/ui.rs:129-147`),
  not a TOML key, so retiring it is a code deletion rather than a config break.
- **Configuration — additive, relay-level.** A `[delivery]` table in `relay.toml`
  replaces them: `submission-timeout-ms`,
  `fence-observation-timeout-ms`, `unreachable-dwell-ms`, the four
  admission-quota keys, and
  `undelivered-warning-ms` plus `undelivered-report-interval-ms`.
  **No key bounds how long a delivery waits for a target that is reachable but
  not ready**, and none may be added. `unreachable-dwell-ms` is not that bound:
  it governs how long a target may be continuously *unreachable* before its
  members resolve, qualifying an observation repeatedly made rather than
  substituting for one never made. `submission-timeout-ms` is an **execution watchdog** over the relay's own
  supervised code, mandatory per the operator's 2026-08-04 call: it bounds how
  long an authorized submission may run, states nothing about target health, and
  exists because every other guard trigger is an event that a blocked-but-alive
  executor never produces. The two undelivered keys govern **reporting only** and
  may not influence any outcome, quota, or scheduling decision. These are relay
  configuration rather than per-coder because they describe the relay's own queue,
  not any coder's behavior. A `scheduling-quantum-bytes` key was specified here
  and is withdrawn along with the cross-target round-robin it budgeted; see
  `design.md`.
- **Relay** — `src/relay/delivery/**` gains the pending queue, the admission-quota
  policy, undelivered-queue reporting, and handover dispatch. Cross-target
  scheduling is deliberately absent: each target is served by its own worker and
  the relay arbitrates between none of them.
- **Transports** — `src/transports/{contract,quiescence,mod,ui}.rs`,
  `src/pty/**`, `src/tmux/**`, `src/acp/**`. All five `TransportImpl` variants
  are covered: Tmux, Acp, and Pty implement the contract, Ui implements it and
  loses its reconnect timer, and Pubsub is rejected synchronously at admission
  with the existing not-implemented error — nothing queued, nothing authorized,
  no terminal outcome and no receipt. Pty's write moves after its wait; Tmux is
  closest to the target shape already.
- **Tests** — a reinstated coalesce-during-wait regression test against the
  relay-owned queue is the relay/62 acceptance criterion; the original was
  deleted with the transport-owned wait it exercised. Per project alpha defaults,
  no test asserts that a removed config key is now rejected.
- **Feature gate** — `src/pty/**` is behind the `pty` Cargo feature, which
  neither `cargo nextest run` nor default clippy builds, and the pre-commit
  `cargo-clippy-pty` hook is file-scoped. Default, `--features pty`, and the ACP
  paths are validated independently.
- **Operator action before restart** — Coordinator prepares both `coders.toml`
  files against the settled key list; the relay fails config load otherwise.
- **Forward dependency** — the relay/transport boundary this lands is
  **in-process and payload-shape-compatible** with transports later running as
  isolated processes. The readiness notification is an injected closure rather
  than a transport→relay call, so the dependency direction is one-way. It is
  **not** wire-safe as it stands: a wire transport would need an acknowledgment
  this contract deliberately omits. `embeddable-runtime-api` (0.10.0, unowned)
  inherits the shape, not a finished wire protocol.
