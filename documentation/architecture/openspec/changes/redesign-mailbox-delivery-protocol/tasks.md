## 1. Neutral protocol crate

- [ ] 1.1 Promote `src/transports/vocabulary.rs` into the neutral delivery
      protocol crate boundary. Add mailbox entry/entry-kind types, target and
      consumer identity, consumer-generation binding, cursor position, and
      `peek`/`declare`/`ack` request/response types alongside the existing
      `PackingUnitId`, `SubmissionEvidence`, `PartitionError`, `SendOutcome`,
      `WorkerReadinessState`, `DeliveryPayloadMode`, `WorkerFailureReason`,
      `ToolCallStatus`, `StructuredEntry`, `LookFreshness`,
      `LookSnapshotSource`, `LookSnapshotPayload`.
- [ ] 1.2 Verify the promoted boundary imports nothing from `crate::relay`,
      `crate::acp`, `crate::tmux`, `crate::pty`, or `crate::transports::ui`.
      Add a compile-time or lint check that fails if a back-edge is
      reintroduced.

## 2. Relay-side mailbox (`src/relay/delivery/`)

- [ ] 2.1 Collapse `QueueEntryState` (`guard.rs`) from `Pending`/
      `Authorized`/`Terminal` to `queued`/`terminal`. Remove `GuardKey`'s
      `(batch, attempt)` composite identity; key the guard by mailbox entry
      sequence number, since acknowledgment is idempotent per entry.
- [ ] 2.2 Add `peek(target, entry_max, canonical_bytes_max)` to
      `AdmissionLedger`, read-only, returning the head contiguous mail run or
      a singleton raw entry, gated on the calling connection's consumer
      generation matching `active_generation_id`.
- [ ] 2.3 Add `declare(target, generation_id, through_seq)` to
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
- [ ] 2.4 Add `ack(target, generation_id, packing_unit_id, evidence)` to
      `AdmissionLedger`. Under the same lock: check `generation_id` first
      (reject on mismatch); look up `packing_unit_id`'s bound range from the
      `declare` record (reject if never declared or already terminalized
      under this generation, no-op if already terminalized and matching);
      record evidence, advance the cursor by exactly the declared range,
      terminalize covered members through the guard's evidence order, and
      release quota.
- [ ] 2.5 Remove `HandoverWindow` (`dispatch/batch.rs`) and its use in
      `dispatch/worker.rs` (construction, `SubmitContext.window`,
      `form_batch`). Remove the held-member slot (`worker.rs:275` and its
      call sites) and `TargetGate`/`gate_target`/`decide_gate`
      (`worker.rs:774-882`).
- [ ] 2.6 Remove `authorize_batch` (`admission.rs:423-456`) and its call site
      (`worker.rs:961-990`). Fold its packing-unit-binding logic
      (`declare_packing_unit`, `record_unit_evidence`,
      `record_evidence_for_member`) into the `declare`/`ack` split from 2.3
      and 2.4 — binding at `declare` time, evidence at `ack` time, matching
      the pre-effect/post-effect split the push model already had.
- [ ] 2.7 Add `active_generation_id` as a durable per-target field in
      `LedgerState`, drawn from a monotonically increasing, never-reused
      per-target-identity sequence (never a recycled or default value, even
      across a full teardown-and-recreation of the target). Add the
      replacement path: require a positive `GenerationFence` verdict for the
      outgoing generation (`src/relay/delivery/fence.rs`, unchanged) before
      flipping it, under the same lock `declare`/`ack` use.
- [ ] 2.8 Add the delivery doorbell: a per-generation `Arc<Notify>`
      constructed at the same point `readiness_changed` is today
      (`worker.rs:210,716-718`), invoked on a mailbox empty-to-non-empty
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
      sequence itself MUST NOT reset on this cleanup.

## 3. Transport-side delivery-loop executors

- [ ] 3.1 Design and implement the delivery-loop executor shape shared by
      all transports (peek → coalesce/render → measure against token budget
      → declare the decided prefix → write it → ack from the write's
      evidence → repeat, woken by doorbell + poll), replacing each
      transport's `WriteItem` FIFO consumer loop (`src/acp/transport.rs`,
      `src/tmux/transport.rs`). Include the failure path: a declared unit
      whose write fails observably MUST still be acked (`NotSubmitted` or
      `SubmissionUnknown`) rather than left to the watchdog when the
      executor is able to report.
- [ ] 3.2 Remove `mailw`, `raww`, and `is_ready_for_handover` from the
      `Transport` trait (`src/transports/contract.rs:274-386`). Spawn the
      delivery-loop executor from each transport's `startup`.
- [ ] 3.3 Enforce one serial delivery executor per transport instance —
      no per-connection executor spawn. In-process scope only; no
      reconnect handling (see `design.md`'s scope note).
- [ ] 3.4 Wire ACP's delivery-loop executor: peek, declare, render
      pane-envelope text, submit via `session/prompt`, ack from partition
      evidence.
- [ ] 3.5 Wire Tmux's delivery-loop executor: peek, pack into token-budget
      prompts, declare each resulting unit, inject, ack from partition
      evidence.
- [ ] 3.6 Wire Pty's delivery-loop executor analogously.
- [ ] 3.7 Wire UI's delivery-loop executor: peek, declare, emit as relay
      stream events through the injected broadcaster closure, ack.
- [ ] 3.8 Wire raw-entry handling into each delivery-loop executor per
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
