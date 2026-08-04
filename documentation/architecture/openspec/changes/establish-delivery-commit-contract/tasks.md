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

- [ ] Add the queue entry state model — `Pending`, `Authorized`, `Terminal` — with `Batch ID`, `Member ID`, and a stable attempt ID
- [ ] Implement admission: atomically reserve per-target and relay-global count and byte quota before returning `queued`
- [ ] Reject at admission an envelope whose canonical payload exceeds the target transport's maximum handover dimensions
- [ ] Reject a `Pubsub` target synchronously at admission with the existing not-implemented error, queueing and authorizing nothing
- [ ] Implement authorization as a relay-local transition that creates the batch's owner in the same atomic operation
- [ ] Implement the relay-owned guard: keyed on `(batch, member, attempt, generation)` at authorization, atomically bound to `PackingUnit ID` at partition
- [ ] Implement the single terminal CAS, releasing admission quota on that transition and nowhere else
- [ ] Implement the guard resolution order once: unit record if present, else `not_submitted` for a member never bound to a unit, else `submission_unknown`
- [ ] Implement the mandatory post-authorization execution watchdog bounded by `[delivery].submission-timeout-ms`, anchored at authorization
- [ ] On elapse, resolve every unresolved member through the guard's evidence order and initiate the generation fence; record `submission_unknown` only where no stronger evidence exists
- [ ] Make unwind, channel closure, task or thread exit, generation replacement, and graceful shutdown all route through that one order; no lifecycle path selects an outcome of its own
- [ ] Make collectors carry guard keys rather than own resolution; remove the `JoinError` branch in `src/relay/delivery/dispatch/outcomes.rs` that returns without producing an outcome
- [ ] Ensure outcome-notification failure is counted and recorded without blocking the terminal transition or the quota release

### Residency and scheduling

- [ ] Implement residency over `Pending` entries only, resolving `expired`, and make it unable to fire against an `Authorized` entry
- [ ] Implement per-target FIFO covering mail and raw as one order
- [ ] Implement byte-budgeted round-robin across targets: canonical payload bytes as the cost unit, exactly one quantum of credit per visit with no carry-over, ineligible targets skipped
- [ ] Form batches against both handover components, stopping at whichever of envelope count or canonical payload bytes binds first
- [ ] Debit the visit's remaining credit at authorization, in the same atomic operation as the `Pending` to `Authorized` transition
- [ ] Revalidate the quantum against the largest byte component when a transport registers or changes its declared maxima, refusing registration rather than admitting an unschedulable transport
- [ ] Make authorization outrank residency expiry within one scheduling iteration, and keep an activity-advanced target unauthorizable
- [ ] Reschedule `Pending` entries to a new generation on respawn; never re-invoke `Authorized` entries
- [ ] Resolve still-`Pending` members `dropped_on_shutdown` on graceful shutdown, and `Authorized` members from evidence

### Readiness

- [ ] Add the level-triggered `can_accept_handover` state to the transport contract, readable on demand
- [ ] Add static maximum handover dimensions in envelope count and canonical payload bytes
- [ ] Move prompt-readiness matching and quiescence observation relay-side, into the `Pending` phase
- [ ] Wire the readiness notification as a relay-provided closure, with subscribe-before-check and a bounded poll backstop
- [ ] Keep the activity signal as a transport-exposed observation the relay reads; delete the classifier's use of it

### Submission evidence

- [ ] Add the typed evidence enum — `Submitted`, `NotSubmitted`, `SubmissionUnknown` — and map undifferentiated errors to `SubmissionUnknown`
- [ ] Make partition deterministic and recorded to the guard before any target-side effect
- [ ] Record one immutable per-unit evidence record before member fan-out, and resume fan-out from it after a panic
- [ ] Resolve an unbound member `not_submitted`, keyed on unit binding rather than on the manner of failure
- [ ] Resolve every member from its own unit's record; remove group-wide outcome application

### Fencing

- [ ] Retain every generation-owned submission and permission executor handle
- [ ] Implement fence acknowledgment as terminate-then-join, in that order
- [ ] Implement the five-step fence state machine: cooperative stop request, bounded cessation observation, non-blocking forced termination, second bounded observation, verdict
- [ ] Keep steps 1 and 3 distinct, so a cooperatively stoppable executor is never force-terminated
- [ ] Make both observations non-blocking with their own clock, never a blocking join, each bounded by `[delivery].fence-join-timeout-ms`
- [ ] Add a generation termination primitive to the transport contract, contracted to positively cease every generation-owned effect path, and require its invocation to return without blocking
- [ ] Implement it per transport: ACP and Pty reap the generation's child; Tmux terminates only its owned client invocations, never the server; UI drops the generation's broadcaster handle and subscriber senders
- [ ] Make the fence positive only on observed cessation, and route both timeout and failure to the negative branch
- [ ] Keep the fence negative when cessation is not observed: admit no replacement for that target, hold its raw barrier, record the condition, and still resolve every member through the guard
- [ ] Block replacement and normal-raw ordering barriers until the fence is positive, while allowing `submission_unknown` to terminalize before it
- [ ] Resolve a submission stopped by the fence before any effect as `not_submitted`

### Raw mode

- [ ] Add the `mode` field to the relay `raww` contract, defaulting to `normal`
- [ ] Implement emergency mode overtaking `Pending` mail and bypassing the readiness gate on Tmux and Pty
- [ ] Reject emergency mode on ACP, UI, and Pubsub with `validation_invalid_params` naming the supported set
- [ ] Make normal raw wait for target-side ordering safety rather than for outcome terminality
- [ ] Add `--mode` to `agentmux raww` and `mode` to the MCP `raww` schema as a plain optional enumerated string, not a nullable union

### Per-transport

- [ ] Pty: move the write after the partition; buffer then write; one unit per member; resolve each member from its own evidence
- [ ] Pty: delete the wedge classifier and the prime wait
- [ ] Tmux: delete the readiness bound, the prime wait, and the quiescence wait; keep per-unit partition and outcomes, which are already correct
- [ ] ACP: remove the staging queue so an authorized batch starts a supervised executor synchronously
- [ ] ACP: record `Submitted` immediately after the framed `session/prompt` write succeeds, before replay-buffer locks or `on_dispatched`
- [ ] ACP: map active-prompt refusal and serialization failure to `not_submitted`, and a stdin write or flush error without proof of zero bytes to `submission_unknown`
- [ ] ACP: retain the client/child thread handle so the generation can be fenced (see `agentmux:todos/relay/128`)
- [ ] ACP: delete the prime timer, `acp_turn_timeout`, and the readiness latch and respawn signal it drove
- [ ] Delete `src/transports/quiescence.rs` and the `WedgeProbe` trait

### Configuration

- [ ] Delete the five per-coder keys and their loader, validation, and default machinery
- [ ] Add the `relay.toml` `[delivery]` table with residency, submission timeout, quantum, fence-join bound, and the four admission-quota keys
- [ ] Validate `scheduling-quantum-bytes` at load against every registered transport's maximum handover dimension
- [ ] Delete `prime_timeout_ms` and `readiness_timeout_ms` from `DeliveryEnvelope`

### Documentation

- [ ] Update operator docs to describe relay-side residency as the single delivery-patience setting, applying to every transport
- [ ] State the crash-recovery limitation: guarantees hold for a surviving relay process and graceful shutdown only
- [ ] Reconcile `session-relay/spec.md` hub prose (requirement total, the partition description advertising prime/wedge timeouts)
- [ ] Refresh the MCP tool inventory after the `raww` schema change: restart the server, verify tool inventory client-side, and record the outcome in the lane handoff

### Tests

- [ ] Remove `#[ignore]` from `pty_envelope_absorbed_during_wait_reaches_the_master`; it passing is the `agentmux:issues/relay/62` acceptance criterion
- [ ] Cover exactly-once resolution under worker panic, collector panic, transport panic mid-partition and after partial submission, closed outcome channel, generation replacement in flight, and graceful shutdown with mixed `Pending`/`Authorized`
- [ ] Cover that quota returns to zero after each of those, and that the per-target FIFO still makes progress
- [ ] Cover fence acknowledgment ordering: a join without termination authority does not complete, and one with it does
- [ ] Cover the execution watchdog: an executor that stays alive and blocked past `submission-timeout-ms` resolves through the guard, releases quota, unblocks the target FIFO, and initiates the fence
- [ ] Cover that the watchdog does not override stronger evidence, and that it produces no failure spelling and no target-health inference
- [ ] Cover the cooperative path: an executor that observes the fenced flag ceases in the first window and the termination primitive never fires
- [ ] Cover the escalation path: the first window elapses, the termination primitive fires, cessation is observed within the second window, and the fence becomes positive
- [ ] Cover the fail-stop path: cessation not observed when the second window elapses leaves the fence negative, blocks replacement, and still resolves every member
- [ ] Cover that a UI generation terminates without an owned child, and that fencing a Tmux generation leaves the server running
- [ ] Cover that an unbound member resolves `not_submitted` under every trigger, and that a recorded `Submitted` is not downgraded by a generation replacement
- [ ] Cover that siblings of one packing unit never receive different outcomes from one evidence record
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
