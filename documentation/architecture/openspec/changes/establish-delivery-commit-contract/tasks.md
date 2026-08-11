# Implementation Tasks

The spec deltas describe the whole contract. This file draws the phase line.

**Phase 1 is the 0.9.0 core.** **Phase 2 is 0.9.x follow-on.** The line is where
it is because the authorization guard cannot be deferred past the `Authorized`
state: quota releases only at the terminal transition, so an unowned `Authorized`
entry leaks quota permanently and blocks the target FIFO.

Validate default, `--features pty`, and the ACP paths independently at every
checkpoint — `src/pty/**` is behind a feature no default gate builds, and the
pre-commit `cargo-clippy-pty` hook is file-scoped.

## Phase 1 — 0.9.0 core

### Relay queue, authorization, and guard

- [x] Add the queue entry state model — `Pending`, `Authorized`, `Terminal` — with `Batch ID`, `Member ID`, and a stable attempt ID
- [x] Implement admission: atomically reserve per-target and relay-global count and byte quota before returning `queued`
- [x] Reject at admission an envelope whose canonical payload exceeds the target transport's maximum handover dimensions
- [x] Reject a `Pubsub` target synchronously at admission with the existing not-implemented error, queueing and authorizing nothing
- [x] Implement authorization as a relay-local transition that creates the batch's owner in the same atomic operation
- [ ] Implement the relay-owned guard: keyed on `(batch, member, attempt)` at authorization, atomically bound to `PackingUnit ID` at partition — creation at authorization is done, and the binding is now a real `PackingUnit ID` recorded before the first target-side effect rather than a boolean standing in for one. What remains is the partition itself. Tmux now reports its own — a budget group is one unit, declared before the paste that carries it — so a multi-member unit is real rather than hypothetical. ACP and Pty are still one member per unit, awaiting their own commits
- [x] Implement the single terminal CAS, releasing admission quota on that transition and nowhere else
- [x] Implement the guard resolution order once: unit record if present, else `not_submitted` for a member never bound to a unit, else `submission_unknown`
- [ ] Route **every** post-authorization pre-partition refusal through the guard's evidence order instead of overriding it. There are three known sites, not one, and they share a shape: an explicit outcome passed to `complete_task_outcome` bypasses the order entirely, so the member reports `failed` where the order would derive `not_submitted` from its being unbound. (a) `submit_task`'s `prepare_coder_write` error arm. (b) Tmux's `paste_group`, when active-pane resolution fails: every member of the flush group is resolved `failed` before any declaration runs. (c) Tmux's `mailw` when the delivery thread has stopped. (b) and (c) became reachable-as-unbound when Tmux started declaring its own partition — before that the relay's singleton declaration had already bound them, so the order would have derived `submission_unknown` and the explicit `failed` was the *better* answer. Now it is the weaker one. Enumerate the remaining sites per transport as each adopts the sink; the ACP and Pty commits will add their own. Both are true — nothing was written — but the spec is explicit that this is not the reporting it wants: a post-authorization refusal SHALL be treated as *a terminal evidence result* (`delivery-quiescence/spec.md:669`), and a pre-partition refusal SHALL terminalize *through the guard* (`:708`). So this is a compliance gap against the contract as already written, not new scope, and needs no spec delta. Preserve the refusal's code and reason as the outcome's `reason_code`/`reason` so the true cause survives the spelling change. Check first whether any test pins `failed` on this path — the spelling is sender-visible
- [x] Implement the mandatory post-authorization execution watchdog bounded by `[delivery].submission-timeout-ms`, anchored at authorization. Arming REQUIRED the transport submission-evidence tasks below, and they hold: all three transports resolve their outcome future at the write boundary rather than at the end of the turn — ACP immediately after the framed `session/prompt` write, Tmux at the `inject_literal_text` invocation, Pty at the buffered `write_all`. ACP's respawn gap needs no exemption either, because `mailw` refuses synchronously with no live runtime rather than holding an authorized member across it. So the bound measures relay-side execution, as specified, and not the agent's inference
- [x] On elapse, initiate the generation fence and terminalize nothing yet; keep accepting unit evidence through the fence windows and terminalize every still-unresolved member through the evidence order at the fence verdict. The fence is advanced from the worker loop rather than awaited, so the collect arm keeps running through both observation windows and a member that resolves in time still reports its own evidence
- [x] Release quota and outcome barriers at terminalization, but release the target's FIFO, raw barrier, and replacement only on a positive fence verdict. Quota release was already bound to the terminal transition alone; what landed here is the other half — a positive verdict builds the replacement generation in place, and a negative one holds the registry entry so none can be elected
- [x] Make unwind, channel closure, task or thread exit, generation replacement, and graceful shutdown all route through that one order; no lifecycle path selects an outcome of its own. Audited against every outcome-selection site in the worker. Unwind resolves `CollectorPanic` and channel closure `ChannelClosed`, both from `collect_outcome`'s join arms; a transport task or thread exiting drops its outcome sender, which arrives as the same channel closure; generation replacement resolves `ExecutionBound` at the fence verdict; graceful shutdown resolves `GracefulShutdown` for the members it finds in flight. Every one of the five reaches `complete_task_outcome_from_trigger`, which takes no outcome and reads the evidence order. Apart from the authorized delivery-path refusals tracked in task 26, the explicit-outcome sites that remain all belong to members that were never authorized — the unreachable dwell, the `Pubsub` refusal, and both construction-failure drains sit before `authorize_member` or apply only to queued and held tasks. Shutdown has three more of them: `complete_task_on_shutdown` spells `dropped_on_shutdown` directly for the held member, for a task received mid-shutdown, and for each task the receiver drain finds queued. Those are the task 41 policy path, not a lifecycle event routing around the order — all three entries are provably `Pending`, and the one order governs members whose delivery responsibility has already transferred. So the claim is that no *authorized* member's outcome is selected by the lifecycle path that reaches it, which is what the audit checked. Explicit outcomes for *authorized* members do exist, but they are synchronous refusals on the delivery path rather than any of the five lifecycle events, so they are outside what this task audited. Their inventory lives in task 26 and grows as each transport adopts the partition sink — do not maintain a second copy of it here
- [x] Make collectors carry guard keys rather than own resolution; remove the `JoinError` branch in `src/relay/delivery/dispatch/outcomes.rs` that returns without producing an outcome
- [x] Ensure outcome-notification failure is counted and recorded without blocking the terminal transition or the quota release. The ordering half was already structural — the terminal CAS and quota release run before any notification — so the work was the counting: both outcome-notification channels discarded their result with `let _ =`. A sender with no attached UI and a sender with no live worker are deliberately **not** counted, being ordinary states rather than failures; a disconnected UI, a draining worker, and an unreadable registry are

### Scheduling

- [ ] Ensure no elapsed-time path can resolve a `Pending` entry whose target is reachable: it leaves that state only by authorization, positively observed transport teardown, sustained unreachability past `[delivery].unreachable-dwell-ms`, or graceful shutdown
- [x] Make the per-target FIFO guarantee explicit and tested: mail and raw as one order, defined as worker-enqueue linearization rather than request or admission order
- [ ] Form batches against both handover components, stopping at whichever of envelope count or canonical payload bytes binds first
- [ ] Keep an activity-advanced target unauthorizable, even when the later observation matches the prompt-readiness template
- [ ] Reschedule `Pending` entries to a new generation on respawn; never re-invoke `Authorized` entries
- [x] Resolve still-`Pending` members `dropped_on_shutdown` on graceful shutdown, and `Authorized` members from evidence. Both halves hold, and the split is structural rather than a rule the shutdown path remembers: a member becomes `Authorized` only after the readiness and health gates, so every task the shutdown path can still reach by another route — the held member, the queue drain, and a task received mid-shutdown — is provably `Pending` and takes the `dropped_on_shutdown` spelling. `Authorized` members are exactly those in flight, and `shutdown_drain` terminalizes them through the guard's evidence order at the fence verdict. A `Pending` entry taking a policy outcome is what the spec permits rather than a violation of the one order, which governs members whose delivery responsibility has already transferred

### Undelivered-queue reporting

- [x] Delete the `expired` outcome from the outcome type, receipt surfaces, inscription set, and CLI/MCP vocabulary; it has no producer — no code carried it, so this is spec-side only and lands at sync
- [x] Emit the periodic undelivered-queue aggregate at `undelivered-report-interval-ms`, carrying global count and bytes plus a per-target breakdown, suppressed entirely when nothing is `Pending`
- [x] Emit a first-crossing warning per target at `undelivered-warning-ms`, deduplicated per target and re-armed when that target's queue empties
- [x] Keep both emissions free of delivery effects: no resolution, no quota release, no scheduling change

### Readiness

- [x] Add the level-triggered `is_ready_for_handover` state to the transport contract, readable on demand
- [x] Add static maximum handover dimensions in envelope count and canonical payload bytes
- [x] Remove the weaker `is_ready` predicate from the transport contract so one readiness question remains, and drop the default body that could answer it wrongly
- [x] Gate authorization on `is_ready_for_handover` during the `Pending` phase, so no batch is authorized for a target that cannot take a handover
- [ ] Keep prompt-readiness evaluation inside each owning transport, feeding `is_ready_for_handover`; the relay compiles no `prompt_regex`, inspects no pane output, and compares no cursor column
- [x] Add `TransportHealth` to the transport contract as a level distinct from readiness, carrying the instant unreachability was first observed
- [x] Require both axes for a delivery attempt: `Healthy` and `is_ready_for_handover`, with healthy-but-unready leaving the member `Pending`
- [x] Add the `[delivery]` dwell threshold and resolve still-`Pending` members through the guard once their target has been continuously unreachable past it, releasing quota on that terminal transition
- [x] Reset the dwell on any return to `Healthy`, so a transient unreachability resolves nothing
- [x] Report health from each transport without a relay dependency: Tmux distinguishes a failed pane observation from an observed non-prompt frame; ACP reads its permanence signal (`is_permanent` / respawn give-up) rather than treating every `Unavailable` as unreachable, so a recoverable respawn gap does not bounce
- [ ] Carry the health level in `look` responses and keep `look` served on an unreachable target; `raww` inherits the write gate through the shared ordered channel
- [x] Propagate a transport's startup failure out of worker construction, so a spawn or init error resolves the triggering task instead of installing an already-dead transport the health gate then holds through the dwell
- [x] Move every transport's `startup` off the async worker through one relay-side chokepoint, rather than deciding per session type from what each implementation happens to do today. Delete Pty's initialization handshake rather than bounding it: the readiness axis already answers when a worker is usable, and a bound whose cleanup joins the stalled thread relocates the hang instead of ending it
- [x] Construct the worker's transport at worker spawn rather than on first write, passing the target member the spawn site already holds, and resolve the triggering task if construction fails. The spawn site now resolves a `WorkerTransportContext` before registering anything, so a target whose member cannot be resolved is reported to that sender synchronously rather than discovered later by a worker — and registering only after resolution is what keeps a failure from leaving an entry with a live sender and no worker behind it. Construction became one function, `build_generation`, used for the worker's first generation and for every replacement a positive fence verdict builds; a target can no longer acquire a second transport kind by being fenced. `TransportImpl` stops being an `Option` in the worker as a result
- [x] Wire the readiness notification as a relay-provided closure the transport invokes, with subscribe-before-check and a bounded poll backstop
- [x] Delete the shared wedge/prime classifier once every transport determines its own readiness, and with it the relay's dependence on a cross-transport quiescence state machine

### Submission evidence

- [x] Add the typed evidence enum — `Submitted`, `NotSubmitted`, `SubmissionUnknown` — and map undifferentiated errors to `SubmissionUnknown`
- [ ] Make partition deterministic and recorded to the guard before any target-side effect. The mechanism is in: `PartitionSink` is the relay-injected seam a transport reports its partition through, and `declare` binds every proposed member or none, under the ledger lock, before the write. Tmux reports through it — its budget group is the unit, declared between the token-budget split and the paste. ACP and Pty do not yet, so `TransportImpl::reports_own_partition` still routes them through the relay's singleton declaration; that predicate exists only to keep each transport's adoption a separate commit and goes away with the last one
- [x] Record one immutable per-unit evidence record before member fan-out, and resume fan-out from it after a panic. The record is unit-owned rather than copied per member — `LedgerState.units` holds one `UnitRecord` per live unit, written once before fan-out and read by every member that terminalizes, then dropped when the last of them does. Disagreement between siblings is no longer representable rather than merely avoided: there is one record and they all read it
- [x] Resolve an unbound member `not_submitted`, keyed on unit binding rather than on the manner of failure
- [ ] Resolve every member from its own unit's record; remove group-wide outcome application

### Fencing

- [x] Supervise the initial ACP bootstrap as its own task rather than awaiting it ahead of the delivery loop, so a worker whose agent never finishes its handshake still reaches its shutdown gate and emits a verdict; give every bootstrap ownership of the runtime it produces from creation through install-or-teardown and readiness publication, since aborting the awaiting task cancels none of that; drop the second, unsupervised establish route on `AcpTransport::startup`
- [x] Break the ACP startup readiness poll on a shutdown request, so a signal mid-startup does not cost the relay its full prime timeout for every member still to come
- [x] Retain every generation-owned submission and permission executor handle, including the agent child of every in-flight ACP bootstrap. Aborting the async wrapper does not cancel the `spawn_blocking` closure, so the closure publishes its child into a per-transport registry the moment it spawns — before the handshake that can hold it there — and clears it as it disposes of the runtime
- [x] Make forced termination a latched state rather than a traversal of what is published at that instant, on both ACP and Tmux. A child spawned or published after the traversal was never signalled at all, and an executor between invocations presented an empty slot; publication now inherits the latch under the same lock the traversal takes, and a request that loses a non-blocking lock attempt is served by whoever holds it
- [x] Implement the five-step fence state machine as the only fence protocol: cooperative stop request, bounded cessation observation, non-blocking forced termination, second bounded observation, verdict
- [x] Keep steps 1 and 3 distinct, so a cooperatively stoppable executor is never force-terminated
- [x] Make both observations non-blocking with their own clock, never a blocking join, each bounded by `[delivery].fence-observation-timeout-ms`
- [x] Add a generation termination primitive to the transport contract, contracted to *initiate* cessation of every generation-owned effect path and return without blocking
- [x] Implement step 3 per transport as initiation only: ACP and Pty signal the child to terminate; Tmux signals its owned client invocations, never the server; UI drops the generation's broadcaster handle and subscriber senders. ACP signals the child of every in-flight bootstrap as well as the steady-state one, which is what ends a handshake the relay has no operation timeout over
- [x] Observe the results in step 4 — child reaped, invocations exited, executor returned — never inside the step 3 call. A bootstrap's record outlives its client, and dropping that client kills and waits the child, so an empty bootstrap registry is a reaping observation rather than a signalling one. Step 3 itself takes no blocking lock and performs no wait on either transport: a Tmux invocation is reaped by polling `try_wait` with the child still reachable, and both the ACP child lock and its bootstrap registry are only ever attempted. What a failed attempt would have reached is served instead by the holder, which re-reads the latch *after* releasing — a check taken while still holding the lock leaves the window where the requester's attempt fails against a holder that has already looked. The ACP registry's handoff re-runs the whole traversal rather than serving the holder's own record, because overlapping bootstraps mean the holder that defeats an attempt is generally not the owner of what that attempt would have reached
- [x] Make a successful primitive invocation not itself acknowledge the fence; only observed cessation does
- [x] Make the fence positive only on observed cessation, and route both timeout and failure to the negative branch
- [x] Keep the fence negative when cessation is not observed: admit no replacement for that target, hold its raw barrier, record the condition, and still resolve every member through the guard. The registry entry *is* the no-replacement guarantee — registration is the election a spawner must win, so a fail-stopped entry that outlives its worker is what no second generation can start beside. `raww` needs no barrier of its own for the same reason: it reaches the target through that same lookup
- [x] Block replacement and raw ordering barriers until the fence is positive, while allowing `submission_unknown` to terminalize before it. Terminalization runs at the verdict whatever its sign, so an unknown is reported without waiting on a positive one; only replacement and further writes are held
- [ ] Make raw wait for target-side ordering safety rather than for outcome terminality, which is the consumer side of the barrier above
- [ ] Resolve a submission stopped by the fence before any effect as `not_submitted`

### Per-transport

- [x] Pty: move the write after the partition; buffer then write; one unit per member; resolve each member from its own evidence
- [x] Pty: delete the wedge classifier and the prime wait
- [x] Tmux: delete the readiness bound, the prime wait, and the quiescence wait; keep per-unit partition and outcomes, which are already correct
- [x] ACP: remove the staging queue so an authorized batch starts a supervised executor synchronously
- [x] ACP: record `Submitted` immediately after the framed `session/prompt` write succeeds, before replay-buffer locks or `on_dispatched`
- [x] ACP: map active-prompt refusal and serialization failure to `not_submitted`, and a stdin write or flush error without proof of zero bytes to `submission_unknown`
- [x] ACP: retain the client/child thread handle so the generation can be fenced (see `agentmux:todos/relay/128`)
- [x] ACP: delete the prime timer, `acp_turn_timeout`, and the readiness latch and respawn signal it drove
- [x] Delete `src/transports/quiescence.rs` and the `WedgeProbe` trait

### Configuration

- [x] Delete the five per-coder keys and their loader, validation, and default machinery
- [x] Add the `relay.toml` `[delivery]` table with submission timeout, fence-observation bound, the four admission-quota keys, and the two undelivered-reporting keys
- [x] Add the `[delivery]` unreachable-dwell key that bounds how long a target may be continuously unreachable before its `Pending` members resolve
- [x] Delete the `scheduling-quantum-bytes` key, its default, range, and load-time validation against transport handover maxima, now that no scheduling policy consumes it
- [x] Delete `prime_timeout_ms` and `readiness_timeout_ms` from `DeliveryEnvelope`

### Documentation

- [x] Update operator docs to state that no setting bounds how long a delivery waits for a reachable-but-unready target on any transport, that `unreachable-dwell-ms` bounds continuous unreachability only, and that `submission-timeout-ms` bounds relay-side execution rather than either wait
- [x] Document that a `Pending` entry for a reachable-but-never-ready target holds its admission quota indefinitely, distinguishing it from a continuously unreachable target whose members resolve past the dwell, and naming per-target quota as the bound on the consequence and the undelivered-queue inscriptions as how to observe it
- [x] State the crash-recovery limitation: guarantees hold for a surviving relay process and graceful shutdown only
- [ ] Reconcile `session-relay/spec.md` hub prose (requirement total, the partition description advertising prime/wedge timeouts)

### Tests

- [x] Reinstate the coalesce-during-wait regression test; it passing is the `agentmux:issues/relay/62` acceptance criterion. Landed as `pty_delivery_writes_every_member_of_a_partitioned_group`, which forces two envelopes into one partitioned group and asserts both bodies reach the writer. It asserts on bytes rather than outcomes because the outcome is precisely what the defect made untrustworthy — a resolved member with no bytes behind it. An earlier framing of this task called for driving the relay queue instead of Pty's; that was wrong. The defect lived in Pty's group resolution and the fix lives in Pty's partition-then-write ordering, so a relay-level test would exercise neither, and a tmux-level one could not reproduce it at all since only Tmux ever committed after its wait
- [ ] Cover exactly-once resolution under worker panic, collector panic, transport panic mid-partition and after partial submission, closed outcome channel, generation replacement in flight, and graceful shutdown with mixed `Pending`/`Authorized`
- [ ] Cover that quota returns to zero after each of those, and that the per-target FIFO still makes progress
- [x] Cover fence acknowledgment ordering: against an executor blocked in a primitive that observes no flag, the first observation window does not complete, and cessation is observed only after the termination primitive has been invoked
- [x] Cover the execution watchdog: an executor that stays alive and blocked past `submission-timeout-ms` initiates the fence and terminalizes nothing at the bound; still-unresolved members terminalize through the guard at the verdict, releasing quota there, and the target's FIFO stays blocked unless that verdict is positive. Landed as `an_executor_blocked_past_the_bound_is_fenced_and_its_member_resolved`, which parks a real ACP executor mid-write by having the agent stop draining its stdin behind a 150 KiB body, and asserts arming, the `submission_timeout`-triggered fence, a `forced` positive verdict, the child's death, a `submission_unknown` member, and the replacement generation a positive verdict releases. Two clauses it does **not** discriminate, stated rather than papered over: whether terminalization happened at the bound or at the verdict, because step 3 frees the parked write and so produces the member's own evidence legitimately inside the second window — the ordering is a genuine race and asserting either way would be asserting it; and the fail-stop half, which belongs to the negative-verdict task below. Teeth checked by disarming the watchdog and watching it fail on the missing arming inscription
- [x] Cover that the watchdog does not override stronger evidence, and that it produces no failure spelling and no target-health inference. `a_long_agent_turn_never_arms_the_execution_watchdog` holds the strongest available form of the first: an agent that takes the prompt and never answers leaves the turn running for many multiples of the bound, and the member — already `Submitted` at the framed write — resolves `delivered` while no watchdog ever arms. The spec's mixed-batch phrasing (one unit `Submitted`, another bound) is not constructible yet, because batches are still one member each until batch formation lands. No failure spelling is asserted directly in the blocked-write test above; target-health inference is not asserted at all, and is not claimed to be
- [x] Cover the cooperative path: an executor that observes the fenced flag ceases in the first window and the termination primitive never fires
- [x] Cover the escalation path: the first window elapses, the termination primitive fires, cessation is observed within the second window, and the fence becomes positive
- [ ] Cover the fail-stop path: cessation not observed when the second window elapses leaves the fence negative, blocks replacement, and still resolves every member. The protocol half is covered in `unit/delivery_fence.rs` against a controllable generation. The worker-level half — the fail-stopped registry entry, the refusal of every later send, and raw inheriting that gate — has no reachable trigger: forcing a negative verdict needs a generation that survives forced termination, and on every real transport termination is what ends it. It wants either a fake transport injectable into the worker loop or a stub that can ignore SIGKILL, neither of which exists
- [ ] Cover that a UI generation terminates without an owned child, and that fencing a Tmux generation leaves the server running
- [ ] Cover that an unbound member resolves `not_submitted` under every trigger, and that a recorded `Submitted` is not downgraded by a generation replacement
- [ ] Cover that siblings of one packing unit never receive different outcomes from one evidence record
- [ ] Cover that a `Pending` entry survives an arbitrarily long wait and is still authorized and delivered when its target finally becomes ready
- [ ] Cover the two axes independently: a healthy-but-unready target holds its member indefinitely, and an unreachable one resolves its member only after the dwell threshold
- [ ] Cover that unreachability shorter than the threshold resolves nothing and the member still delivers afterward, and that a target flapping across the threshold boundary resolves each member exactly once
- [ ] Cover that `look` is served on an unreachable target and reports the health level, while `raww` to the same target is gated
- [ ] Cover that no elapsed duration resolves a `Pending` entry whose target is reachable, releases its quota, or produces a receipt for it
- [x] Cover the warning dedup: many entries crossing together emit one inscription for their target, and the target re-arms after its queue empties
- [x] Cover that the aggregate is suppressed when nothing is `Pending`, and that neither emission changes any outcome, quota, or scheduling position
- [ ] Assert the teeth of the ordering and absence tests by reverting each mechanism and confirming the test fails

## Phase 2 — 0.9.x follow-on

- [ ] Convert `TransportImpl::Ui` to the contract and delete its reconnect timeout constant and builder
- [ ] Extend the guard surface beyond the minimum
- [ ] Durable crash recovery, tracked separately

## Interim exceptions carried by Phase 1

The specs describe the end state. This is the one place the core knowingly does
not yet reach it — an implementation-phase exception rather than a property of
the specified contract.

- **`TransportImpl::Ui` keeps its reconnect timer** until Phase 2 lands, so timer
  retirement is not yet universal.
