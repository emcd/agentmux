## 1. Neutral protocol crate

- [x] 1.1 Promote `src/transports/vocabulary.rs` into `src/protocol`, the
      neutral delivery protocol crate boundary. Add mailbox entry/entry-kind
      types, target and
      consumer identity, consumer-generation binding, cursor position, and
      `peek`/`declare`/`ack` request/response types alongside the existing
      `PackingUnitId`, `SubmissionEvidence`, `PartitionError`, `SendOutcome`,
      `WorkerReadinessState`, `DeliveryPayloadMode`, `WorkerFailureReason`,
      `ToolCallStatus`, `StructuredEntry`, `LookFreshness`,
      `LookSnapshotSource`, `LookSnapshotPayload`.
- [x] 1.2 Verify the promoted boundary imports nothing from `crate::relay`,
      `crate::acp`, `crate::tmux`, `crate::pty`, or `crate::transports` —
      the whole of `transports`, not only its `ui` module, since the
      boundary sits below it and every part of it is one side of the
      inversion. Add a compile-time or lint check that fails if a back-edge
      is reintroduced.

## 2. Relay-side mailbox (`src/relay/delivery/`)

Every task in this section is separately landable. The property that makes
one landable is not that it changes nothing, but that it leaves exactly one
delivery path in place: it may add state nothing reads yet, or move where
an artifact is built, but it may not retire the push path or stand a second
consumer beside it. A task that would do either belongs in section 3.

2.7 through 2.10 are inert in the stronger sense — nothing in production
reads what they add. 2.5 is not: it moves payload construction ahead of the
write, and the push path then delivers what it built. That is deliberate.
A shadow that built a parallel payload nothing delivered would prove
nothing about the artifact the executor later consumes, which is the one
thing the shadow exists to establish.

- [x] 2.1 Collapse `QueueEntryState` (`guard.rs`) from `Pending`/
      `Authorized`/`Terminal` to `queued`/`terminal`. Remove `GuardKey`'s
      `(batch, attempt)` composite identity; key the guard by mailbox entry
      sequence number, since acknowledgment is idempotent per entry.
- [x] 2.2 Add `peek(target, entry_max, canonical_bytes_max)` to
      `AdmissionLedger`, read-only, returning the head contiguous mail run or
      a singleton raw entry, gated on the calling connection's consumer
      generation matching `active_generation_id`.
- [x] 2.3 Add `declare(target, generation_id, through_seq)` to
      `AdmissionLedger`. Under the ledger's existing single lock: check
      `generation_id` against `active_generation_id` first (reject on
      mismatch without effect); validate the named range starts exactly at
      cursor + 1, is contiguous, and does not exceed the mailbox's actual
      contents (reject on any violation without effect); **reject if the
      target already has an outstanding declared-and-unacked unit,
      regardless of the requested range** (RG round-2 finding: without
      this, two `declare` calls for the same unacked range both pass the
      other checks unchanged, minting two `PackingUnitId`s for one entry);
      on success, mint a `PackingUnitId`, create the guard, and bind it to
      the named entries. This is the guard's creation point (relocated
      from `authorize_batch`), not `ack`.
- [x] 2.4 Add `ack(target, generation_id, packing_unit_id, evidence)` to
      `AdmissionLedger`. Under the same lock: check `generation_id` first
      (reject on mismatch); look up `packing_unit_id`'s bound range from the
      `declare` record (reject if never declared or already terminalized
      under this generation, no-op if already terminalized and matching);
      record evidence, advance the cursor by exactly the declared range,
      terminalize covered members through the guard's evidence order, and
      release quota.
- [x] 2.5 Wire admission-to-mailbox enqueue: every admitted entry gains a
      relay-built `MailboxPayload` and becomes peekable. Nothing peeks
      until the cutover in section 3; the push model keeps delivering, but
      it delivers **the stored payload** rather than one of its own. This
      adds no delivery path and removes none.
      - **One payload per entry, built exactly once, at enqueue.** The
        push path during shadow and the delivery-loop executor after
        cutover both consume that same stored artifact. A second payload
        built at write time would put a different envelope on the wire
        than the one the mailbox holds, and the shadow would then prove
        nothing about the artifact the executor later delivers.
      - **One timestamp, stamped at enqueue.** `build_delivery_message`
        already takes `created_at` from its caller, but
        `build_ui_envelope` reads the clock itself; that read must move
        out so both paths are stamped by the enqueue rather than by
        whoever renders. The `Date` header therefore moves from write time
        to enqueue time — confirm no requirement or test depends on
        write-time stamping.
      - **One envelope-metadata inscription per task**, emitted where the
        payload is built. `emit_envelope_metadata_inscription` fires today
        from the coder envelope path at write time; moving payload
        construction without moving it emits metadata describing an
        envelope other than the one delivered, and emitting at both points
        double-counts.
      - Build the payload from the `AsyncDeliveryTask` and a supplied
        timestamp, touching no transport, so the seam sits at the delivery
        worker's task intake — the first point the relay holds the task,
        and still before any transport contact.
      - Do NOT place the enqueue inside `admit`. Admission is a
        reservation pass across a request's targets and no
        `DeliveryEnvelope` exists there; envelopes are built per target
        downstream.
      - Mailbox order does not depend on where the enqueue lands, because
        `admit` already assigns the sequence. Placement is therefore free
        to follow the payload rather than constraining it.
      - A terminal-outcome receipt bypasses admission, so it holds no
        ledger entry and no sequence. `enqueue` refuses it `NotAdmitted`,
        and the receipt MUST continue to reach its sender outside the
        mailbox rather than being dropped on that refusal.
      - Entries admitted before this lands, and any entry whose payload
        cannot be built, MUST still resolve through the push path; a
        failed enqueue may not strand a member.
- [x] 2.6 Prove the shadow before anything depends on it. Two claims need
      establishing, and neither is demonstrated by 2.5 landing.
      - **The mailbox stays bounded.** The argument is that the push path
        still terminalizes every entry and `terminalize` retires the
        entry's mailbox position, so the cursor advances. That is reasoned
        from the retirement semantics added in 2.1-2.4, not observed.
        Establish against the full suite: a target's mailbox returns to
        empty once its entries are delivered; the cursor advances rather
        than stalling behind a retired position; no entry is delivered
        twice or left unresolved; quota returns to its pre-send level.
        Each needs its own observable, because they are separate state
        released by one transition and either can return while another
        leaks. **Quota restoration** is not implied by the mailbox
        emptying: report the reservation an entry joined, in both
        components, and require it to cover that entry alone. **Not
        delivered twice** is a claim about the target, not the ledger:
        count the representations that actually reached it, since the
        terminal transition suppresses a second *resolution* and would
        hide a second *write* behind one outcome. **Not left unresolved**
        is the wait itself, which must fail rather than pass when an
        entry never completes. And require the outcome to be `delivered`:
        every other figure here is equally satisfied by a relay that
        refuses every write, since a refused member is admitted,
        enqueued, and terminalized like any other.
      - **The delivered artifact is the stored one.** The payload and
        `Date` observed at enqueue are the payload and `Date` that reach
        the target, exactly one envelope-metadata inscription is emitted
        per task, and push-path delivery behaviour is otherwise unchanged.
        This is what makes the shadow evidence about the executor's future
        input rather than about a parallel artifact nothing delivers.
      Treat a mailbox that grows without bound under sustained delivery,
      or a delivered envelope that differs from the stored one, as a gate
      on the cutover rather than a defect to fix later.
- [x] 2.7 Add `active_generation_id` as a durable per-target field in
      `LedgerState`, drawn from a monotonically increasing, never-reused
      per-target-identity sequence (never a recycled or default value, even
      across a full teardown-and-recreation of the target). Add the
      replacement path: require a positive `GenerationFence` verdict for the
      outgoing generation (`src/relay/delivery/fence.rs`, unchanged) before
      flipping it, under the same lock `declare`/`ack` use.
      - **The datum's whole value is that a stale identifier cannot come back
        around**, so the sequence must outlive the mailbox it governs: hold it
        beside the mailboxes rather than on one, or 2.10's cleanup reclaims it
        along with the cursor and a recreated target restarts at the first
        value.
      - **There is no default generation.** The placeholder this replaces
        defaulted every target to the first identifier, which any caller could
        name; a target nobody has claimed must refuse `peek`, `declare` and
        `ack` rather than admit whichever caller guessed.
      - The fence is driven outside the lock — it is async and the ledger lock
        is never held across an await — so the verdict is presented to the flip.
        Name the outgoing generation in the same call, or a verdict obtained for
        a generation that has since been replaced is spent on whichever
        generation happens to be active by then.
      - A replacement MUST resolve what the outgoing generation had declared and
        not acknowledged. The fence establishes that execution ceased, never
        whether it took effect first, so re-serving those entries risks writing
        a message twice — and leaving the declaration outstanding refuses every
        later declaration for the target with a unit nobody can acknowledge.
        Hand the resolved members back rather than swallowing them: each still
        owes its sender a terminal outcome, and this call emits none.
- [ ] 2.8 Add the delivery doorbell: a per-generation `Arc<Notify>`
      constructed at the same point `readiness_changed` is today
      (`worker/run.rs`, `worker/spawn.rs`), invoked on a mailbox
      empty-to-non-empty
      transition, paired with a bounded poll backstop mirroring
      `ASYNC_WORKER_POLL_INTERVAL_MS`.
- [ ] 2.9 Add the policy-admission-snapshot behavior: confirm (or add, if
      not already the case) that an admitted entry carries no live policy
      reference re-checked later; a policy change affects only new
      admissions.
- [ ] 2.10 Confirm mailbox/cursor/generation-sequence cleanup rides the
      existing worker-registry reap path
      (`src/relay/delivery/async_worker/registry.rs`) when a generation is
      torn down without replacement; add cleanup there if it is not already
      covered for the state added in 2.2-2.7. The monotonic generation-id
      sequence itself MUST NOT reset on this cleanup: the seam for giving a
      target up without replacing it exists (`release_consumer_generation`),
      and what remains is wiring it to the reap. Two obligations fall on that
      wiring rather than on the seam. The reap MUST name the generation it is
      reaping and treat a refusal as the correct answer: a reap runs behind the
      target it reaps, so one for a generation already replaced would otherwise
      clear an owner that is still consuming. And it MUST resolve the target's
      entries before releasing, because the release is what admits the next
      claimant, and a claimant that inherited an outstanding declaration could
      neither acknowledge nor declare past it.

## 3. Production cutover

This whole section lands as one change. No task in it is separately
mergeable, because each removes or replaces a mechanism the others depend
on: retiring the push path before an executor can peek leaves no delivery
path at all, and standing an executor up beside a live push path would
write every entry twice. Removing `mailw`/`raww` from the trait likewise
forces every transport to be wired in the same change rather than one at a
time.

The section is therefore sized by that constraint rather than by
convenience. Splitting it requires a migration switch that routes per
target between the two paths — a decision to take deliberately, not by
letting a tranche boundary fall somewhere convenient.

- [ ] 3.1 Design and implement the delivery-loop executor shape shared by
      all transports (peek → coalesce/render → measure against token budget
      → declare the decided prefix → write it → ack from the write's
      evidence → repeat, woken by doorbell + poll), replacing each
      transport's `WriteItem` FIFO consumer loop (`src/acp/transport/`,
      `src/tmux/transport/`). Include the failure path: a declared unit
      whose write fails observably MUST still be acked (`NotSubmitted` or
      `SubmissionUnknown`) rather than left to the watchdog when the
      executor is able to report.
- [ ] 3.2 Give readiness and unreachable-dwell an explicit owner in the
      executor. Both behaviours live in `decide_gate` today and are deleted
      by 3.4, so this task exists to keep them from being lost in the gap:
      a target continuously `Unreachable` past
      `[delivery].unreachable-dwell-ms` MUST still resolve its entries
      `not_submitted`, which the `transport-abstraction` capability
      requires normatively, and an unready target MUST leave its entries
      `queued` rather than being written to. Neither may be left to fall
      out of 3.1's shape by accident; name where each is judged, and
      confirm the dwell is measured over continuous unreachability rather
      than restarted by an unobservable reading.
- [ ] 3.3 Remove `mailw`, `raww`, and `is_ready_for_handover` from the
      `Transport` trait (`src/transports/contract/transport.rs`). Spawn the
      delivery-loop executor from each transport's `startup`.
- [ ] 3.4 Remove `HandoverWindow` (`dispatch/batch.rs`) and its use in
      `dispatch/worker/` (construction, `SubmitContext.window`,
      `form_batch`). Remove the held-member slot (`worker/run.rs` and its
      call sites) and `TargetGate`/`gate_target`/`decide_gate`
      (`worker/gate.rs`, `worker/submit.rs`).
- [ ] 3.5 Remove `authorize_batch` (`admission/authorize.rs`) and its call
      site (`worker/submit.rs`). Complete the transfer of its
      packing-unit-binding logic (`declare_packing_unit`,
      `record_unit_evidence`, `record_evidence_for_member`) to the
      `declare`/`ack` split from 2.3 and 2.4 — binding at `declare` time,
      evidence at `ack` time, matching the pre-effect/post-effect split the
      push model already had. `declare_packing_unit` also backs
      `PartitionSink` (`src/relay/delivery/partition.rs`), the seam a
      transport calls to report its own partition, so that caller MUST be
      moved to `declare` in the same change rather than left pointing at a
      removed function.
- [ ] 3.6 Enforce one serial delivery executor per transport instance —
      no per-connection executor spawn. In-process scope only; no
      reconnect handling (see `design.md`'s scope note).
- [ ] 3.7 Wire ACP's delivery-loop executor: peek, declare, render
      pane-envelope text, submit via `session/prompt`, ack from partition
      evidence.
- [ ] 3.8 Wire Tmux's delivery-loop executor: peek, pack into token-budget
      prompts, declare each resulting unit, inject, ack from partition
      evidence.
- [ ] 3.9 Wire Pty's delivery-loop executor analogously.
- [ ] 3.10 Wire UI's delivery-loop executor: peek, declare, emit as relay
      stream events through the injected broadcaster closure, ack.
- [ ] 3.11 Wire raw-entry handling into each delivery-loop executor per
      `transport-contracts`' `Relay raww transport behavior`: a peeked raw
      singleton is declared as its own packing unit, written using the same
      per-transport injection mechanics as today, then acked.

## 4. Documentation

- [ ] 4.1 Update `src/relay/README.md` (or the nearest subsystem README) to
      describe the pull-model mailbox in place of the push-model
      handover/authorization description.
- [ ] 4.2 Update operator-facing documentation per `delivery-quiescence`'s
      `Quiescence Documentation` and `Async Queue Growth Risk Disclosure`
      requirements: quiescence is transport-owned, no bound governs waiting
      to be peeked/written, admission quota bounds mailbox growth.

## 5. Testing

- [ ] 5.1 Port or rewrite the exactly-once/guard tests from
      `establish-delivery-commit-contract` against the two-state model:
      resolve-exactly-once under a delivery-loop-executor panic, duplicate
      acknowledgment convergence, declared-but-no-evidence member resolves
      `submission_unknown` regardless of trigger, undeclared member never
      reaches the guard at all.
- [ ] 5.2 Add a test that genuinely races two `ack` calls for overlapping
      entries under different generations, per the project's existing
      finding that uniqueness assertions which never contest the gate are
      tripwires, not demonstrations (see `agentmux:issues/relay/69`'s
      carried-forward finding).
- [ ] 5.3 Add a test proving the revocation/in-flight serialization for
      **both** `declare` and `ack`: a call already inside the lock for the
      outgoing generation completes before a concurrently-requested
      replacement can flip `active_generation_id`, and a genuinely late
      `declare` or `ack` for a superseded generation is rejected without
      effect.
- [ ] 5.4 Add a test proving `declare` range validation is enforced, not
      trusted: a `declare` call naming a non-contiguous range, a range not
      starting at cursor + 1, or a range past the mailbox's actual contents
      is rejected without binding anything — closing the "arbitrary
      `through_seq`" hole a free-form `ack` would otherwise leave.
- [ ] 5.4a Pin the at-most-one-outstanding-declaration invariant (RG
      round-2 finding): `declare(1..5)` followed by a second
      `declare(1..5)` before the first is acked — same generation, same
      target — MUST reject the second call without minting a second
      `PackingUnitId`. Repeat for a non-overlapping second range (e.g.
      `declare(6..8)` while `1..5` is still outstanding) to confirm the
      rule is a total order, not a narrower overlap check. Confirm
      `declare(6..10)` succeeds once `1..5` has been acked.
- [ ] 5.5 Add a test proving `ack` cannot reference an undeclared or
      already-terminalized-elsewhere `packing_unit_id`: a caller with a
      valid generation binding but no matching `declare` record cannot
      advance the cursor or release quota for any entry.
- [ ] 5.6 Add a test for the raw-singleton peek rule: `peek` never returns
      mail past an unpeeked raw entry, and returns a raw head entry alone
      even when it would fit inside `entry_max`/`canonical_bytes_max`
      alongside following mail.
- [ ] 5.7 Add a test for partial acknowledgment: peek N, declare and ack a
      unit covering M < N, confirm the remainder is still peekable,
      undeclared, and unaffected.
- [ ] 5.8 Add a doorbell-miss test: suppress the notify, confirm the poll
      backstop still delivers within its bound.
- [ ] 5.9 Add a shutdown-distinction test: with a mix of undeclared and
      declared-but-unacked entries at shutdown, confirm undeclared entries
      resolve `dropped_on_shutdown` and declared entries resolve through
      the guard's evidence order — never the reverse.
- [ ] 5.10 Add a generation-epoch test: tear down a target's worker-registry
      entry (mailbox/cursor cleaned up), recreate a session under the same
      name, and confirm the new `active_generation_id` does not equal any
      previously issued for that target identity.
- [ ] 5.11 Re-run the ACP `Busy`-after-one-envelope scenario that motivated
      this proposal and confirm it no longer produces a spurious
      `not_submitted` for members 2..N of a formerly-relay-sized batch.
- [ ] 5.12 Confirm `agentmux:issues/relay/69` and `agentmux:todos/relay/134`
      (carried-forward `establish-delivery-commit-contract` residue) are
      each either resolved by this design or explicitly re-derived against
      the pull model, per that residue's own "do not port these verbatim"
      instruction, before this change is archived.
- [ ] 5.13 Confirm `Mailbox Payload Custody` has reached the live
      `delivery-quiescence` spec before this change is archived, and not
      merely that a sync was offered. The requirement is delta-only until
      then, and decision record 0004 in `documentation/decisions/` names it
      as where its standing rule lives — a citation that resolves against
      nothing until the sync happens.
      Tracked here rather than in the notebook because `opsx-archive` reads
      this file and warns on an unchecked box, which puts the warning at the
      moment the mistake would be made; nothing reads the notebook at
      archive time. Neither `openspec validate --strict` nor
      `verify-openspec-deltas.py` substitutes: the first checks that a delta
      is well formed and the second that a MODIFIED delta retains what it
      replaces, and a requirement that never reaches a live spec passes both.
