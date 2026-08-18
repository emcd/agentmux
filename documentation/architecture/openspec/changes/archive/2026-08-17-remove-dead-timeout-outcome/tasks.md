# Implementation Tasks

**Do not archive before `establish-delivery-commit-contract`.** That change's
deltas are what remove the surviving `SendOutcome::Timeout` citations from
`delivery-quiescence`, `transport-contracts`, and `transport-abstraction`.
Archiving this change first would publish a live spec set in which active
requirements name a variant the code no longer defines.

An intermediate revision of this file relaxed this on the grounds that the delta
targets are disjoint, so neither order produces a merge conflict. That is true
and insufficient. Disjoint targets rule out a *conflict*; they say nothing about
whether the resulting live spec set is *coherent*. The reasoning that replaced
the constraint — that the affected requirements are already stale about deleted
bounds, so one more wrong detail is harmless — conflated two different defects.
A requirement describing behavior that has since changed is drift a reader can
discover; a requirement naming a type that no longer exists is mechanically
false.

Task 4.1 is the enforcement point. It carries a fourth permitted exception for
live requirements the delivery-contract change has not yet archived, so that the
sweep can be run and reasoned about at any time rather than only at the end. That
exception is a **staging allowance, not an archive allowance**: run task 4.1
after the delivery-contract change archives, and its fourth category must be
**empty**. A non-empty fourth category is an archive-readiness failure for this
change, not a tolerated residue.

**FE implements all of it, including the relay-side code.** The enum lives in
`src/transports/`, its consumers in `src/relay/` and `src/tui/`, and the enum
cannot be deleted independently of its consumers — the compiler forces one
commit rather than a stack. Groups 1 and 2 touch backend-lane files by
arrangement, not by handoff; the backend lane reviews rather than implements.

## 1. Delete the variant and its relay-side renderings

- [x] 1.1 Delete the `Timeout` variant from `SendOutcome` in `src/transports/vocabulary.rs`
- [x] 1.2 Delete the `Timeout` arm from the non-delivered outcome match in `src/relay/delivery/async_worker.rs`
- [x] 1.3 Delete the `Timeout` arm from the receipt-body outcome wording, letting the existing catch-all cover it
- [x] 1.4 Delete the `Timeout` arm from `emit_sender_delivery_outcome_event`, which removes the sole producer of the `timeout` stream spelling
- [x] 1.5 Drop `"timeout"` from `labels_an_outcome` and from the roster its doc comment describes. Settled with the backend lane, per `design.md`: the function strips reason codes that restate an outcome, so keeping the entry would silently discard a causal `timeout` code rather than prevent a contradiction

## 2. Update the relay tests that pin the vocabulary

- [x] 2.1 Retarget both async-worker task fixtures onto the `target_unreachable_result` triple from `dispatch/worker.rs` verbatim — `NotSubmitted` with `delivery_target_unreachable` and its reason string. Neither fixture asserts the outcome word (one pins receipt routing and keying, the other receipt non-recursion), and both members are unbound, so the outcome is not the discriminator. Taking a real production triple whole, rather than pairing a surviving variant with the retired `delivery_prime_timeout` / `target_never_settled` codes, avoids replacing one fiction with a fresher one — those two codes have no production producer left
- [x] 2.2 Remove `SendOutcome::Timeout` from the exhaustive outcome-label assertion — both the variant array and its match arm — keeping the match exhaustive so the next vocabulary change still cannot pass unnoticed
- [x] 2.3 Narrow the integration assertion in `tests/integration/session_relay_stream/ui_delivery.rs` that currently accepts either `timeout` or `failed`, so it names the one outcome the path now produces

## 3. Update the TUI consumption

- [x] 3.1 Delete the `Timeout` arm from the chat-result outcome mapping in `src/tui/state/history.rs`
- [x] 3.2 Delete `"timeout"` from the stream-outcome match and add `"not_submitted"` and `"submission_unknown"`, so the relay's terminal spellings stop falling through to the unknown-outcome placeholder
- [x] 3.3 Delete `"timeout"` from the terminal-message tracking match and add the same two spellings. This set is the guard against a terminal event that arrives *before* the queued acknowledgement — clearing an already-pending id does not depend on the outcome, since any non-`routed` phase removes it, so the defect being fixed is the race and not a failure to clear
- [x] 3.4 Cover each new outcome with an **event-before-acknowledgement** race test: deliver the terminal `not_submitted` (and separately `submission_unknown`) stream event first, then the queued acknowledgement, and assert the message does not land in pending. A test that clears an already-pending id passes today and cannot prove this fix

## 4. Confirm the removal is total

- [x] 4.1 Sweep the live specs and all sources for the **bare** `timeout` outcome spelling as well as the qualified `SendOutcome::Timeout`, and confirm every remaining hit falls under exactly one permitted exception: archived material; the `add-opencode-compose-readiness-contract` delta noted in `proposal.md`; a timeout *setting* rather than an outcome; or a live requirement that `establish-delivery-commit-contract` has not yet archived **and** either removes outright or replaces with text that does not reintroduce the timeout spelling. Verify that second condition against the delta text rather than assuming it — a MODIFIED requirement whose replacement reintroduces the spelling is a real hit, not an excused one. The qualified form alone is not sufficient — the two requirements this change adds deltas for were both found only by the bare-word sweep. Run this **after** the delivery-contract change archives, per the sequencing constraint above, at which point the fourth category must be empty; a non-empty fourth category fails archive readiness for this change. **Deliberately left open at merge.** Its source-side half is done and clean — no `SendOutcome::Timeout` and no `timeout` outcome spelling survives anywhere in `src/` or `tests/`. Its live-spec half cannot honestly run until the delivery-contract change archives, so this task stays incomplete by design rather than by omission, and is the reason this change merges before it archives. **Run after `establish-delivery-commit-contract` archived (2026-08-17); the fourth category is now empty by construction, so only the first three apply.** Full sweep found zero remaining hits of either form in `documentation/architecture/openspec/specs/`, after applying this change's own deltas (`Relay Stream Event Contract` and `TUI Delivery State Mapping` MODIFIED; `ACP Stop-Reason Outcome Mapping` REMOVED in whole, not just its `timeout` bullet — the requirement's other two mappings are independently false per its own delta's `Reason`) and one correction made during the delivery-contract archive pass that anticipated this task: `ACP Stop-Reason Outcome Mapping`'s `acp_turn_timeout` bullet, trimmed there rather than left for here since it directly blocked this sweep. Two adjacent findings surfaced by the same sweep were **not** timeout-outcome hits and are out of this task's scope — `mcp-tool-surface` and `choice-decisions` each carried a stale live mention of the deleted `prime-timeout-ms`/`readiness-timeout-ms` *config keys* (a timeout setting, not an outcome, and pre-dating both this change and the delivery-contract one) — fixed separately, recorded for the record rather than claimed as part of this task's own scope
- [x] 4.2 Update the delivery outcome vocabulary list in `src/tui/README.md` to match the corrected spec. Leave the `relay_timeout` connection status on the same page alone — it names a relay reachability state, not a delivery outcome
- [x] 4.3 Confirm no source or test still references `acp_stop_cancelled`, whose requirement this change removes as unimplemented. Expected to be a no-op that documents the removal was safe rather than a code change

## Correction (Coordinator, 2026-08-18)

Task 4.1's claim above that the source-side half was "done and clean — no
`SendOutcome::Timeout` and no `timeout` outcome spelling survives anywhere in
`src/` or `tests/`" was false when written and is still false: a comment in
`src/relay/delivery/async_worker/terminal.rs:199` reads "...rather than as
`failed` or `timeout`", a bare-word hit the sweep missed. BE found this
2026-08-12, generalizing from a single non-gating AuxBE note on BE's own
stack, and routed it to FE as site 4 of an inventory. FE fixed it inside a
held correction commit; when that commit was dropped 2026-08-18 because
unrelated work in `establish-delivery-commit-contract`'s reconnect-wait
removal (`1f40608`, merged `c060eca`) had obsoleted most of its other fixes,
this one went with it. FE noticed the loss while dropping the commit, not
while sweeping for it, and handed the one-word fix back to BE to fold into
their next commit on that file. Recorded here rather than edited into the
original sentence, since this document is a historical record of what task
4.1 found at archive time, not a live claim.
