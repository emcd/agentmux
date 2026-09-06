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
- [x] 2.3 Add `declare(target, generation_id, range)` to
      `AdmissionLedger`, the range naming both ends. Under the ledger's
      existing single lock: check
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
- [x] 2.8 Add the delivery doorbell: a per-generation `Arc<Notify>`
      constructed at the same point `readiness_changed` is today
      (`worker/run.rs`, `worker/spawn.rs`), invoked on a mailbox
      empty-to-non-empty
      transition, paired with a bounded poll backstop mirroring
      `ASYNC_WORKER_POLL_INTERVAL_MS`.
      - **Registered as a generation is built, not beside
        `readiness_changed`.** That notifier belongs to the worker and outlives
        every generation the worker goes on to build; a doorbell belongs to the
        generation that waits on it. So it is built in `build_generation` — the
        same function that injects the readiness notifier into the transport —
        which gives it the generation's lifetime rather than the worker's, and
        makes a replacement that forgot to register one impossible rather than
        merely unlikely.
      - **The `Arc<Notify>` is `protocol::DeliveryDoorbell`, which task 1.1
        already landed.** Its `ring` retains a signal when nobody is waiting, so
        a ring made before this generation's executor exists is not a lost
        notification. What the relay holds is not that handle but an opaque
        closure over it, which is the injected-closure shape the requirement
        names and keeps the relay from holding a type belonging to the side it
        signals.
      - **Rung on the transition a peek can see, which is narrower than the
        mailbox gaining an entry and narrower than the mailbox having been
        empty.** A run starts at the cursor, so an entry filling a position
        behind one that is admitted and not yet filled leaves every peek
        returning nothing. Ringing there tells a consumer about a run it cannot
        see; the empty-to-non-empty reading does exactly that and is then silent
        for the entry that finally exposes the run. Both readings are pinned
        against by the same test.
      - **The lock is released before the ring**, the one place in this
        subsystem where a lock is dropped before its operation finishes. A
        doorbell is foreign code and the ledger lock is a non-reentrant
        `std::sync::Mutex`.
      - **Only a registration displaces a registration.** Neither the reap nor
        the fenced replacement clears one, and that is the whole of the
        arrangement's safety rather than tidiness deferred. Both run *behind* the
        target they act on, so a successor can already have registered by the
        time either reaches the ledger; a clear would take the successor's
        doorbell with nothing left to put one back, since a generation registers
        once as it is built. RG found this on the reap (round 1): the
        consumer-generation naming that protects the mailbox does not cover it
        and cannot yet, because until the executors claim a generation every
        target answers `None` and a late reap matches. Leaving a stale
        registration costs one closure per target identity the process has
        served — the bound `generations` already carries — and nothing rings one,
        because an entry reaches a mailbox only through a worker's own intake.
      - **The poll backstop is the one the worker already runs.** The doorbell
        arm and the readiness arm are backstopped by the same
        `ASYNC_WORKER_POLL_INTERVAL_MS` tick; a second timer would be duplicate
        machinery for one bound.
      - Nothing waits on a doorbell until the executors arrive, so a ring's only
        trace today is `doorbell_rung` on the enqueue inscription. It is
        asserted against a live relay rather than only in the ledger's tests,
        which is what catches a generation that registered nothing — a failure
        the ledger's own tests cannot see, since they register their own.
- [x] 2.9 Add the policy-admission-snapshot behavior: confirm (or add, if
      not already the case) that an admitted entry carries no live policy
      reference re-checked later; a policy change affects only new
      admissions.
      - **Already the case, and nothing was added to make it so.** Every
        `authorize_*` call, every `AuthorizationContext`, and every
        `load_authorization_context` lives in `src/relay/handlers/`,
        `src/relay/mod.rs` or `src/relay/lifecycle.rs` — the request boundary.
        `src/relay/delivery/` names `authorization` only in prose, and its
        `authorize_batch` is the push model's own all-or-none batch
        transition, which is about custody rather than about who may send.
      - **What an admitted entry does carry is a snapshot and a name, neither
        of which is a decision.** `AsyncDeliveryTask` holds a cloned
        `BundleConfiguration`; `AdmittedEntry` and `MailboxSlot` hold nothing
        policy-shaped at all. `BundleMember.policy_id` rides along, but every
        occurrence of it under `src/relay/delivery/` is a struct-literal
        initializer — the field is written and never read, so the delivery
        side holds the *name* of a policy and no means of resolving it.
      - **The prospective half is structural too**:
        `load_authorization_context` reads `policies.toml` from disk on each
        call and memoizes nothing, so what a request is judged against is the
        file as it stood when that request arrived.
      - Pinned behaviorally rather than by a lint, because the requirement is
        about behavior: an entry held under a long unreachable dwell survives a
        total tightening of `send` to `none`, while the next request is refused
        under it. Neither half stands alone — the survival is satisfied by a
        relay that never re-read the file, and the refusal says nothing about
        the entry already in the mailbox.
      - Teeth: memoizing `load_authorization_context` fails the refusal;
        resolving the held member after admission fails both the still-waiting
        and the no-terminal-outcome assertions, which read one transition from
        its two sides. The queued-nothing assertion is a disambiguation of the
        waiting count rather than a new claim — the refusal-admits-nothing
        property has its own coverage in the admission cluster.
- [x] 2.10 Confirm mailbox/cursor/generation-sequence cleanup rides the
      existing worker-registry reap path when a generation is torn down
      without replacement; add cleanup there if it is not already covered for
      the state added in 2.2-2.7. The monotonic generation-id sequence itself
      MUST NOT reset on this cleanup.
      - **Confirmed absent, not merely unverified.** Nothing removed a
        `mailboxes` entry anywhere, so a target's cursor and next position
        outlived every teardown. The cleanup had to be added rather than
        found.
      - The reap is `unregister_worker`, and it is already the
        teardown-without-replacement event: a worker whose generation is
        fenced and rebuilt in place keeps its registration, so an entry
        leaving means the target is going rather than changing hands.
      - **The reap names the generation it gives up**, which the registry
        entry carries — not the ledger's current answer, since a registry
        entry outlives individual consumer generations and the ledger would
        name whoever holds the target at reap time.
      - **Reach the ledger after the registry lock is released.** Nesting them
        would introduce a lock ordering nothing else in the subsystem
        observes. The window this opens is the one the naming exists for.
      - **A reap that finds entries still admitted gives the target up but
        keeps the mailbox.** It knows those entries by id rather than as the
        tasks that produced them, so it could terminalize them and report none
        — silence for every waiting sender. Resolution is owed by the teardown
        path holding the tasks.
      - That obligation is met everywhere except the two construction-failure
        paths, and their exception is deliberate and pre-existing: a worker
        whose transport could not be built — on its first attempt or on a
        replacement after a positive fence verdict — unregisters *before*
        draining, because the registry lock is what keeps a send from landing
        in a receiver nothing will poll. Their reap can only retain, so the
        reclamation is retried after the drain, naming nothing since the first
        pass already gave the generation up.
      - **Pair the drain and the retry in one function rather than at each
        call site.** They were split when this landed, and only the first of
        the two paths got the retry; the second left an empty mailbox and its
        cursor behind for good, so a target recreated under that name kept
        stale positions. Splitting them means a third such path inherits the
        obligation without inheriting the code.

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

- [x] 3.1 Design and implement the delivery-loop executor shape shared by
      all transports (peek → coalesce/render → measure against token budget
      → declare the decided prefix → write it → ack from the write's
      evidence → repeat, woken by doorbell + poll), replacing each
      transport's `WriteItem` FIFO consumer loop (`src/acp/transport/`,
      `src/tmux/transport/`). Include the failure path: a declared unit
      whose write fails observably MUST still be acked (`NotSubmitted` or
      `SubmissionUnknown`) rather than left to the watchdog when the
      executor is able to report.
- [x] 3.2 Give readiness and unreachable-dwell an explicit owner in the
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
- [x] 3.3 Remove `mailw`, `raww`, and `is_ready_for_handover` from the
      `Transport` trait (`src/transports/contract/transport.rs`). Spawn the
      delivery-loop executor from each transport's `startup`.
- [x] 3.4 Remove `HandoverWindow` (`dispatch/batch.rs`) and its use in
      `dispatch/worker/` (construction, `SubmitContext.window`,
      `form_batch`). Remove the held-member slot (`worker/run.rs` and its
      call sites) and `TargetGate`/`gate_target`/`decide_gate`
      (`worker/gate.rs`, `worker/submit.rs`).
- [x] 3.5 Remove `authorize_batch` (`admission/authorize.rs`) and its call
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
- [x] 3.6 Enforce one serial delivery executor per transport instance —
      no per-connection executor spawn. In-process scope only; no
      reconnect handling (see `design.md`'s scope note).
- [x] 3.7 Wire ACP's delivery-loop executor: peek, declare, render
      pane-envelope text, submit via `session/prompt`, ack from partition
      evidence.
- [x] 3.8 Wire Tmux's delivery-loop executor: peek, pack into token-budget
      prompts, declare each resulting unit, inject, ack from partition
      evidence.
- [x] 3.9 Wire Pty's delivery-loop executor analogously.
- [x] 3.10 Wire UI's delivery-loop executor: peek, declare, emit as relay
      stream events through the injected broadcaster closure, ack.
- [x] 3.11 Wire raw-entry handling into each delivery-loop executor per
      the `raww` capability's `Relay raww transport behavior`: a peeked raw
      singleton is declared as its own packing unit, written using the same
      per-transport injection mechanics as today, then acked.

## 4. Documentation

- [x] 4.1 Update `src/relay/README.md` (or the nearest subsystem README) to
      describe the pull-model mailbox in place of the push-model
      handover/authorization description.
- [x] 4.2 Update operator-facing documentation per `delivery-quiescence`'s
      `Quiescence Documentation` and `Async Queue Growth Risk Disclosure`
      requirements: quiescence is transport-owned, no bound governs waiting
      to be peeked/written, admission quota bounds mailbox growth.

## 5. Testing

- [x] 5.1 Port or rewrite the exactly-once/guard tests from
      `establish-delivery-commit-contract` against the two-state model:
      resolve-exactly-once under a delivery-loop-executor panic, duplicate
      acknowledgment convergence, declared-but-no-evidence member resolves
      `submission_unknown` regardless of trigger, undeclared member never
      reaches the guard at all.
- [x] 5.2 Add a test that genuinely races two `ack` calls for overlapping
      entries under different generations, per the project's existing
      finding that uniqueness assertions which never contest the gate are
      tripwires, not demonstrations (see `agentmux:issues/relay/69`'s
      carried-forward finding).
- [x] 5.3 Add a test proving the revocation/in-flight serialization for
      **both** `declare` and `ack`: a call already inside the lock for the
      outgoing generation completes before a concurrently-requested
      replacement can flip `active_generation_id`, and a genuinely late
      `declare` or `ack` for a superseded generation is rejected without
      effect.
- [x] 5.4 Add a test proving `declare` range validation is enforced, not
      trusted: a `declare` call naming a non-contiguous range, a range not
      starting at cursor + 1, or a range past the mailbox's actual contents
      is rejected without binding anything — closing the "arbitrary
      `through_seq`" hole a free-form `ack` would otherwise leave.
- [x] 5.4a Pin the at-most-one-outstanding-declaration invariant (RG
      round-2 finding): `declare(1..5)` followed by a second
      `declare(1..5)` before the first is acked — same generation, same
      target — MUST reject the second call without minting a second
      `PackingUnitId`. Repeat for a non-overlapping second range (e.g.
      `declare(6..8)` while `1..5` is still outstanding) to confirm the
      rule is a total order, not a narrower overlap check. Confirm
      `declare(6..10)` succeeds once `1..5` has been acked.
- [x] 5.5 Add a test proving `ack` cannot reference an undeclared or
      already-terminalized-elsewhere `packing_unit_id`: a caller with a
      valid generation binding but no matching `declare` record cannot
      advance the cursor or release quota for any entry.
- [x] 5.6 Add a test for the raw-singleton peek rule: `peek` never returns
      mail past an unpeeked raw entry, and returns a raw head entry alone
      even when it would fit inside `entry_max`/`canonical_bytes_max`
      alongside following mail.
- [x] 5.7 Add a test for partial acknowledgment: peek N, declare and ack a
      unit covering M < N, confirm the remainder is still peekable,
      undeclared, and unaffected.
- [x] 5.8 Add a doorbell-miss test: suppress the notify, confirm the poll
      backstop still delivers within its bound.
      `tests/unit/delivery_executor.rs`. The entry is placed *after* the
      executor has parked, which is the whole of it: an entry seeded before
      the loop starts is drained on the first pass, before any wait, and
      such a test passes against an executor with no backstop at all. The
      bound is deliberately generous and nothing asserts elapsed time — the
      claim is that correctness does not depend on a ring, not that the
      timer is punctual. The stub mailbox gained a `place` for this, since
      seeding through its constructor cannot express an arrival while a
      consumer is already idle.
- [x] 5.9 Shutdown distinction: covered by
      `tests/integration/acp/generation_fence/shutdown.rs`, which parks a
      declared member inside its framed write, holds an undeclared one
      behind it, sends a real SIGTERM, and asserts the undeclared member
      resolves `dropped_on_shutdown` while the declared one does not.
      **The declared member's spelling is deliberately not asserted**, and
      should not be: the fence's forced step frees the parked write, so the
      executor's own evidence legitimately resolves it and the answer moves
      with that timing. Pinning a spelling there buys a flaky test.
      **The remaining case is structurally unreachable.** A declared member
      still unresolved when the post-fence guard drain runs — so that the
      guard rather than the executor terminalizes it — needs an executor
      that survives forced termination. No real transport provides one, and
      the injectable-transport seam that would is the same one deferred for
      the fail-stop path. A probe on that drain reports zero members every
      time.
- [x] 5.10 Generation epoch: covered by
      `src/relay/delivery/async_worker/reap.rs`'s
      `unregistering_an_owned_entry_reaps_its_target_and_a_foreign_one_reaps_nothing`,
      which binds a consumer generation to a registered worker, unregisters
      the owned entry so the reap rides it, re-claims under the same target,
      and asserts the new identifier is greater than the one given up. It
      pins the negative half too — a non-owner's unregister gives up
      nothing. Note that `resolve_queued_tasks_and_reclaim` is *not* this
      path: it names no generation, so it is refused by a claimed target and
      serves only the construction-failure case where none was ever held.
- [x] 5.11 Re-run the ACP `Busy`-after-one-envelope scenario that motivated
      this proposal and confirm it no longer produces a spurious
      `not_submitted` for members 2..N of a formerly-relay-sized batch.
      `tests/unit/delivery_executor.rs`, driven at the executor's readiness
      gate rather than through a live ACP child: `AcpDeliveryWriter::is_ready`
      admits only `Available`, so a worker mid-turn fails the same gate the
      stub fails, and the gate is what decides the fate of the entries
      behind the one it took. The assertion is an absence — members two and
      three carry no outcome at all while the target is busy, and nothing
      behind it is left declared — with delivery after readiness returns as
      the positive control that the absence is suspension rather than loss.
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
- [x] 5.14 Settle `agentmux:todos/backend/7`: `declare` was specified as
      `declare(target, generation_id, through_seq)` by `Mailbox Submission
      Declaration` and by task 2.3, while the implementation takes a range
      carrying both ends. Resolved in favour of the range, and the
      requirement now says so. Naming only the run's last position leaves
      the start derived rather than stated, so a caller cannot express a
      wrong one, which makes the requirement's own out-of-position
      rejection unrepresentable and task 5.4 unwritable — a rule no code
      can enforce. The caller is not guessing the value either: `peek`
      returns the cursor it read at, so a well-formed declaration echoes
      back the position the relay just served and the check compares the
      two views of it. Task 2.3's wording is corrected here, and the same
      signature in `transport-abstraction`'s delivery-loop sequence and in
      `src/transports/contract/executor.rs` with it. `Mailbox Submission
      Declaration` is ADDED by this change rather than MODIFIED, so no live
      requirement had to be reconciled.
- [ ] 5.15 Run the retention audit against this change at its **pre-archive**
      state and confirm every dropped scenario, per
      `agentmux:todos/backend/8`, which holds the recorded baseline, the
      extraction command, and the per-scenario classification. The count is
      **41** at time of writing; that note's title still says 42, which was
      the figure before `Keep the upward-signal closure pinned by a scenario`
      restored one. Re-running is not optional at archive: a MODIFIED delta
      replaces the whole requirement, so the drop set moves whenever another
      change archives into a requirement this one targets — with no delta
      file edited and therefore no commit firing the `lint-openspec-deltas`
      hook. That is the one shape the hook structurally cannot see.
      Sweep backwards as well as forwards while there: drops are found live
      spec into delta, but a citation going stale is found only delta prose
      into what the change deletes, and that backwards sweep is what caught
      the single real loss in the early audit while both gates were green.
      **The recorded baseline is short by the renamed requirements.**
      `verify-openspec-deltas.py` matches a delta's requirement to the live
      spec by name, so the three in this change's `RENAMED` block are
      compared against nothing and every scenario they drop is unreported —
      **18** of them, against the script's own **44**. Audit those by hand,
      or with `.auxiliary/scribbles/renamed-drops.py`, and reconcile both
      figures before checking this box. The corrected baseline is recorded on
      `agentmux:todos/backend/8`; the script defect is
      `agentmux:issues/openspec/1`.
- [x] 5.16 Settle the guard evidence order's unreachable first rung, per
      `agentmux:todos/backend/9`. The rung deriving a member's outcome from
      its packing unit's recorded evidence was live under the push model,
      where the relay recorded unit evidence before fanning out to members;
      the pull model's acknowledgment records and resolves under one ledger
      acquisition, leaving no state for it to read. Deleted, along with the
      unit record it read and the map holding it, which had no other
      consumer. The two-phase acknowledgment that would reintroduce the
      window is rejected in
      `documentation/decisions/0005-no-two-phase-acknowledgment.md`, and the
      `Guard resolution order` section now states that the order takes no
      rung above its two and points at that record.

Tasks 5.12 through 5.16 are archive preconditions rather than testing work,
and they are recorded here rather than only in the notebook for one reason:
`opsx-archive` reads this file and warns on an unchecked box, which puts the
warning at the moment the mistake would be made. Nothing reads the notebook
at archive time, so an obligation living only there warns whoever thinks to
look — which, months later, is nobody.
