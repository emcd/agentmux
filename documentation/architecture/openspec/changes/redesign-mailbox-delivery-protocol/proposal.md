# Change: Redesign Relay Delivery as a Pull-Model Mailbox

## Why

The current delivery design has the relay guess at consumer-internal state it
cannot see. `HandoverWindow` bounds a batch the relay itself sizes; a
held-member slot models one member the relay believes a transport can accept
next; `is_ready_for_handover` is a level the relay authorizes against, then
invokes synchronously. Each piece was locally justified, but together they
make the relay a scheduler for capacity it does not own.

The guess is demonstrably wrong, not merely inelegant. ACP publishes `Busy` on
accepting the first envelope of a batch and refuses every later one in the
same batch with `not_submitted`, so members 2..N receive a positive
non-delivery claim exactly where a correct per-member gate would have held
them un-authorized instead. Widening the relay-side batch does not fix this:
a producer cannot correctly size a batch for a consumer it cannot see, no
matter how the batch is bounded. A second, independent symptom exists on
Tmux, where a packing unit's membership is decided by delivery-thread timing
rather than by a declared boundary.

This proposal replaces the push model with a pull model: **the relay holds
custody of an ordered, per-target mailbox; transports poll it.** A transport
calls `peek` to read a contiguous prefix without committing to anything,
renders and measures what it read against its own budget, writes a prefix of
what it peeked, and calls `ack` to advance the relay's cursor for exactly the
entries it wrote. The relay no longer authorizes, batches, or gates on a
readiness level it reads from the transport — a busy transport simply does
not peek, and an entry that goes unacked stays queued, in order, for the next
peek. This removes an entire class of relay-side inference (batch sizing,
handover-window admission, stale-readiness-between-check-and-authorization)
by removing the relay decision point it was inference *for*.

The design was reviewed six times against AuxBE, converged on the operator's
own simplifications, and the operator authorized converting it into this
proposal on 2026-08-29 (`agentmux:ideas/21`). It normatively resolves the four
gaps that review left open — revocation-vs-in-flight-acknowledgment
serialization, doorbell durability, policy-change-while-queued disposition,
and mailbox persistence/pruning — rather than deferring them past this
proposal's approval.

## What Changes

- Replace the `Pending`/`Authorized`/`Terminal` queue-entry state machine with
  a two-state `queued`/`terminal` model. There is no authorization event: an
  admitted entry is immediately visible to `peek` and stays queued until an
  `ack` advances the cursor past it.
- Add `peek(target, entry_max, canonical_bytes_max)`: a read-only relay
  operation returning the head contiguous run of mailbox entries within the
  given bounds, advancing nothing. A raw-kind entry at the head is always
  returned alone.
- Add `ack(target, generation_id, through_seq, evidence)`: advances the
  target's cursor and terminalizes exactly the members through `through_seq`,
  from per-member `SubmissionEvidence` the transport supplies. Partial
  acknowledgment — writing and acking fewer entries than were peeked — is
  ordinary, not an exception requiring its own rule.
- Delete `HandoverWindow`, the held-member slot, `authorize_batch`, and
  `is_ready_for_handover` as a relay-side authorization precondition. Delete
  the stale-readiness terminal-outcome class it produced. Readiness
  evaluation (prompt-readiness templates included) stays exactly where it is
  specified today — transport-owned — but now gates the transport's own
  decision to peek and write, not a relay authorization.
- Fold `raww` into the mailbox as its own entry kind rather than a separate
  push call or a second channel. `peek` returns a raw entry only as a
  singleton, which is what keeps it a batch barrier without a rule anyone has
  to remember to enforce elsewhere.
- Introduce **consumer generation** as a durable per-target datum: exactly one
  `active_generation_id` is admitted at a time, checked on every `peek` and
  `ack`; replacement requires the existing `GenerationFence` five-step
  mechanism to reach a **positive** verdict first (reused, not reinvented,
  from `transport-abstraction`'s transport-lifecycle fencing); and one serial
  delivery executor per transport instance persists across reconnect, so
  same-generation reconnect is never itself grounds for a second concurrent
  writer.
- Serialize revocation against in-flight `ack` handling under the same
  single-lock ledger discipline the admission guard already uses, so an
  already-in-flight `ack` cannot commit after a replacement generation is
  admitted, and a replacement is never admitted while an `ack` for the old
  generation could still be mid-flight.
- Introduce the delivery doorbell as a notify-only signal — no data, no
  custody — reusing the existing injected-closure pattern
  (`transport-abstraction`'s upward-signal scenario) rather than new
  machinery; a missed doorbell only delays the next `peek`, backstopped by
  the existing bounded poll.
- Promote the neutral vocabulary already living in `src/transports/
  vocabulary.rs` into the crate boundary both delivery directions (relay to
  transport for `look`, transport to relay for `peek`/`ack`) depend on
  without a back-edge; add the module-boundary requirement that says what it
  may and must not hold.
- Normatively resolve, not defer: revocation/ack serialization mechanism
  (above), doorbell durability (deliberately ephemeral, rebuilt per
  generation, correctness never depends on it), policy-change-while-queued
  (admission-time snapshot, prospective only), and mailbox persistence/
  pruning (stays in-memory and non-durable as today; bounded by the existing
  per-target/global admission quota, no new pruning mechanism).

## Non-Goals

- No decoupled, out-of-process transport-host protocol. That is
  `agentmux:ideas/transports/5`, a later, separately-scoped change that
  depends on ownership work landing first. This proposal is in-process only;
  it designs the IPC boundary in (the neutral crate has no back-edge either
  direction) without building IPC.
- No change to per-transport injection mechanics: how a Tmux pane receives
  text, how an ACP `session/prompt` request is framed, how a Pty master is
  written to. Those stay exactly as specified; only who calls when, and
  through which operation, changes.
- No change to `look`. `look-and-stream-events` already specifies `look` as
  independent of delivery ordering, and this proposal keeps it that way.
- No change to `choice-decisions`. It has no coupling to the deleted
  admission machinery today and needs none introduced.
- No durable mailbox persistence across a relay crash. The existing
  in-process-only recovery scope limitation is preserved, not widened.
- No dedup of at-least-once duplicates. Accepted per the operator's prior
  decision: an agent may occasionally see a message twice; building
  dedup machinery to avoid that is out of scope.

## Impact

- Affected specs:
  - `delivery-quiescence` (primary — queue model, guard, observability,
    receipts, recovery scope all restated in mailbox/cursor/generation
    vocabulary; new peek/write/ack/generation/doorbell/policy/retention
    requirements added)
  - `transport-abstraction` (`Transport Interface Contract` and `Transport
    Handover Capacity and Readiness` replaced; neutral-crate module-boundary
    requirement added; `Transport Generation Fencing and Termination
    Authority` is reused unchanged and is not deltaed)
  - `transport-contracts` (`Prompt-Readiness Template Gating` and `Relay raww
    transport behavior` restated for the pull model; per-transport injection
    substance unchanged)
- Affected code:
  - `src/relay/delivery/dispatch/worker.rs` (`HandoverWindow` use, held-member
    slot, `TargetGate`/`gate_target`/`decide_gate`, `authorize_batch` call
    site)
  - `src/relay/delivery/dispatch/batch.rs` (`HandoverWindow` itself)
  - `src/relay/delivery/admission.rs` (`ADMISSION_LEDGER`, `authorize_batch`,
    packing-unit binding — retained and re-scoped to bind at `ack` rather
    than at authorization)
  - `src/relay/delivery/guard.rs` (`QueueEntryState`, `GuardKey` — collapse to
    two states, generation-checked ack path)
  - `src/transports/contract.rs` (`Transport` trait: `mailw`/`raww`/
    `is_ready_for_handover` removed; consumer-generation binding and a
    peek/ack client surface added)
  - `src/transports/vocabulary.rs` (promoted to the neutral protocol crate)
  - `src/acp/transport.rs`, `src/tmux/transport.rs`, `src/pty/transport.rs`
    (`WriteItem` FIFO becomes each transport's own serial delivery-loop
    executor, calling `peek`/`ack` instead of receiving `mailw`/`raww`)
- Sequencing:
  - `agentmux:todos/backend/2` (process-global singleton removal) and the
    refactor phase (`linecheck`, TUI/relay test splits, ACP transport
    decomposition) proceed independently of this proposal's review.
  - Implementation should land against `admission.rs`/`worker.rs` as
    decomposed by the in-flight refactor pass, not against their
    pre-decomposition shape.
  - `agentmux:issues/pty/5` and `todos/pty/14` (Pty anchor theme's lead
    delivery-bug items) are gated on this proposal landing, per
    `agentmux:coordination/general/18`.
- Source design notes:
  - `agentmux:ideas/21` is the design source of truth, including its
    "Considered and rejected" section (a `claim`/lease step was designed
    across three revisions and dropped; do not re-propose it).
  - `agentmux:ideas/transports/5` is read-alongside context for the IPC
    boundary this proposal designs in but does not build.
