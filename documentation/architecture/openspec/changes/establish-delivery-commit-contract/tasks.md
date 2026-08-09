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
- [ ] Implement the relay-owned guard: keyed on `(batch, member, attempt)` at authorization, atomically bound to `PackingUnit ID` at partition — creation at authorization is done; the `PackingUnit ID` binding lands with partition, and stands in meanwhile on whether the member was handed to a transport
- [x] Implement the single terminal CAS, releasing admission quota on that transition and nowhere else
- [x] Implement the guard resolution order once: unit record if present, else `not_submitted` for a member never bound to a unit, else `submission_unknown`
- [ ] Implement the mandatory post-authorization execution watchdog bounded by `[delivery].submission-timeout-ms`, anchored at authorization. Arming REQUIRES the transport submission-evidence tasks below: until a transport records `Submitted` at write time, its outcome future resolves only after the target has finished responding, so a bound anchored at authorization measures the agent's inference and fences a healthy target mid-turn. Land the two together
- [ ] On elapse, initiate the generation fence and terminalize nothing yet; keep accepting unit evidence through the fence windows and terminalize every still-unresolved member through the evidence order at the fence verdict. Deleted with the watchdog trigger; lands again with arming
- [ ] Release quota and outcome barriers at terminalization, but release the target's FIFO, raw barrier, and replacement only on a positive fence verdict
- [ ] Make unwind, channel closure, task or thread exit, generation replacement, and graceful shutdown all route through that one order; no lifecycle path selects an outcome of its own
- [x] Make collectors carry guard keys rather than own resolution; remove the `JoinError` branch in `src/relay/delivery/dispatch/outcomes.rs` that returns without producing an outcome
- [ ] Ensure outcome-notification failure is counted and recorded without blocking the terminal transition or the quota release

### Scheduling

- [ ] Ensure no elapsed-time path can resolve a `Pending` entry whose target is reachable: it leaves that state only by authorization, positively observed transport teardown, sustained unreachability past `[delivery].unreachable-dwell-ms`, or graceful shutdown
- [x] Make the per-target FIFO guarantee explicit and tested: mail and raw as one order, defined as worker-enqueue linearization rather than request or admission order
- [ ] Form batches against both handover components, stopping at whichever of envelope count or canonical payload bytes binds first
- [ ] Keep an activity-advanced target unauthorizable, even when the later observation matches the prompt-readiness template
- [ ] Reschedule `Pending` entries to a new generation on respawn; never re-invoke `Authorized` entries
- [ ] Resolve still-`Pending` members `dropped_on_shutdown` on graceful shutdown, and `Authorized` members from evidence

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
- [ ] Construct the worker's transport at worker spawn rather than on first write, passing the target member the spawn site already holds, and resolve the triggering task if construction fails
- [x] Wire the readiness notification as a relay-provided closure the transport invokes, with subscribe-before-check and a bounded poll backstop
- [x] Delete the shared wedge/prime classifier once every transport determines its own readiness, and with it the relay's dependence on a cross-transport quiescence state machine

### Submission evidence

- [x] Add the typed evidence enum — `Submitted`, `NotSubmitted`, `SubmissionUnknown` — and map undifferentiated errors to `SubmissionUnknown`
- [ ] Make partition deterministic and recorded to the guard before any target-side effect
- [ ] Record one immutable per-unit evidence record before member fan-out, and resume fan-out from it after a panic
- [ ] Resolve an unbound member `not_submitted`, keyed on unit binding rather than on the manner of failure
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
- [ ] Keep the fence negative when cessation is not observed: admit no replacement for that target, hold its raw barrier, record the condition, and still resolve every member through the guard. The negative registry state was deleted with the watchdog trigger, and shutdown exercises no replacement; lands again with arming
- [ ] Block replacement and normal-raw ordering barriers until the fence is positive, while allowing `submission_unknown` to terminalize before it
- [ ] Resolve a submission stopped by the fence before any effect as `not_submitted`

### Raw mode

- [ ] Add the `mode` field to the relay `raww` contract, defaulting to `normal`
- [ ] Implement emergency mode overtaking `Pending` mail and bypassing the readiness gate on Tmux and Pty
- [ ] Reject emergency mode on ACP, UI, and Pubsub with `validation_invalid_params` naming the supported set
- [ ] Make normal raw wait for target-side ordering safety rather than for outcome terminality
- [ ] Add `--mode` to `agentmux raww` and `mode` to the MCP `raww` schema as a plain optional enumerated string, not a nullable union

### Per-transport

- [x] Pty: move the write after the partition; buffer then write; one unit per member; resolve each member from its own evidence
- [x] Pty: delete the wedge classifier and the prime wait
- [x] Tmux: delete the readiness bound, the prime wait, and the quiescence wait; keep per-unit partition and outcomes, which are already correct
- [ ] ACP: remove the staging queue so an authorized batch starts a supervised executor synchronously
- [ ] ACP: record `Submitted` immediately after the framed `session/prompt` write succeeds, before replay-buffer locks or `on_dispatched`
- [ ] ACP: map active-prompt refusal and serialization failure to `not_submitted`, and a stdin write or flush error without proof of zero bytes to `submission_unknown`
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

- [ ] Update operator docs to state that no setting bounds how long a delivery waits for a reachable-but-unready target on any transport, that `unreachable-dwell-ms` bounds continuous unreachability only, and that `submission-timeout-ms` bounds relay-side execution rather than either wait
- [ ] Document that a `Pending` entry for a reachable-but-never-ready target holds its admission quota indefinitely, distinguishing it from a continuously unreachable target whose members resolve past the dwell, and naming per-target quota as the bound on the consequence and the undelivered-queue inscriptions as how to observe it
- [ ] State the crash-recovery limitation: guarantees hold for a surviving relay process and graceful shutdown only
- [ ] Reconcile `session-relay/spec.md` hub prose (requirement total, the partition description advertising prime/wedge timeouts)
- [ ] Refresh the MCP tool inventory after the `raww` schema change: restart the server, verify tool inventory client-side, and record the outcome in the lane handoff

### Tests

- [ ] Remove `#[ignore]` from `pty_envelope_absorbed_during_wait_reaches_the_master`; it passing is the `agentmux:issues/relay/62` acceptance criterion
- [ ] Cover exactly-once resolution under worker panic, collector panic, transport panic mid-partition and after partial submission, closed outcome channel, generation replacement in flight, and graceful shutdown with mixed `Pending`/`Authorized`
- [ ] Cover that quota returns to zero after each of those, and that the per-target FIFO still makes progress
- [x] Cover fence acknowledgment ordering: against an executor blocked in a primitive that observes no flag, the first observation window does not complete, and cessation is observed only after the termination primitive has been invoked
- [ ] Cover the execution watchdog: an executor that stays alive and blocked past `submission-timeout-ms` initiates the fence and terminalizes nothing at the bound; still-unresolved members terminalize through the guard at the verdict, releasing quota there, and the target's FIFO stays blocked unless that verdict is positive
- [ ] Cover that the watchdog does not override stronger evidence, and that it produces no failure spelling and no target-health inference
- [x] Cover the cooperative path: an executor that observes the fenced flag ceases in the first window and the termination primitive never fires
- [x] Cover the escalation path: the first window elapses, the termination primitive fires, cessation is observed within the second window, and the fence becomes positive
- [ ] Cover the fail-stop path: cessation not observed when the second window elapses leaves the fence negative, blocks replacement, and still resolves every member
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
- [ ] Add the independent supervised writer for Pty emergency raw, with a defined interleaving rule
- [ ] Extend emergency raw to overtake in-flight execution, which depends on that writer
- [ ] Extend the guard surface beyond the minimum
- [ ] Expose emergency raw mode in the TUI raww surface
- [ ] Durable crash recovery, tracked separately

## Interim exceptions carried by Phase 1

The specs describe the end state. These are the two places the core knowingly
does not yet reach it. Both are implementation-phase exceptions rather than
properties of the specified contract.

- **`TransportImpl::Ui` keeps its reconnect timer** until Phase 2 lands, so timer
  retirement is not yet universal.
- **No transport provides a separately supervised writer**, so the clause
  permitting emergency raw to bypass an in-flight submission is unreachable and
  emergency raw always waits for target-side ordering safety. The requirement is
  written against the end state so that adding the writer in Phase 2 does not
  require reopening it.
