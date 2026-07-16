# Tasks: Deliver async terminal send outcomes back to the sender

## 0. Archive gate (READ FIRST)

Per operator constraint (carried in the Coordinator dispatch), this change SHALL
NOT archive into the live spec ahead of the implementation shipping. Every task
in sections 1-4 — the sender receipt across the coder transports, the `relay.log`
completeness, the mis-ack contract, AND the regression test — MUST be merged
before archive. Do not archive on the strength of the proposal docs alone.

Submission-time rejection is intentionally absent from these tasks: absent-target
`validation_unknown_target` and present-but-incapable `validation_unsupported_operation`
are already normative and shipped under the unified registry requirement, and
"offline is a state, not absence" already keeps configured-but-not-ready targets
routable. This change does not restate or re-test that surface.

## Implementation status (Phase 1 — BE relay-side core)

Phase 1 landed the relay-side core: the receipt marker + sender return route on
`AsyncDeliveryTask`, the spawn hook at the single terminal-resolution chokepoint
(`complete_task_outcome`), non-recursion, best-effort drop-if-not-routable, the
`relay.log` floor, and the coder-transport regression plus delivered-no-receipt
and drop-if-not-routable integration tests. Receipts already render and deliver
through every coder transport generically (proven by the tmux regression test).

Phase 2 (ACP portion) landed at `6fbfb36` (merged `f6d94a4`): flush-barrier
turn rendering, zero quiet-window for receipts resolved to ACP targets, empty
choice-decider sessions on receipts, `DeliveryEnvelope.is_receipt` propagation
through `build_coder_envelope`/`build_ui_envelope`, and the `src/acp/README.md`
receipt-rendering section. Reviewed and approved by Reviewer General.

Phase 2 (Pty portion) landed at `4fa89a0` (merged `7716815`): the pty-write
receipt marker line and the `src/pty/README.md` section — see the dedicated
write-up below.

Still deferred (not blocking; the core is unbroken and functional):
- **2.3 Tmux portion + 5.1b Tmux portion** — the same receipt marker line for
  Tmux pane rendering, and the matching transport README update. No
  Tmux-specialist lane exists currently; this is an open question for the
  operator rather than a dispatchable task today.
- **4.4 dedicated non-recursion test** — non-recursion is enforced structurally
  (the `is_receipt` gate is the first check at the single spawn site) and is
  exercised at runtime by the delivered receipt in the regression test; a
  standalone assertion is deferred because a *non-delivered* receipt cannot be
  constructed end-to-end without wedging the very sender pane the assertion reads.
- **Full cross-bundle receipt e2e (review follow-up)** — a real
  sender-bundle -> wedging-target-in-another-bundle test over `serve_connection`
  with a two-bundle catalog and two tmux runtimes. Deferred: the sender-route
  invariant it would prove is now locked by a narrower unit test (below); the
  full end-to-end harness (first to weave `serve_connection`'s multi-bundle
  catalog with real tmux panes across two runtimes) is a later, heavier add.
- **§0 archive** — gated until all phases ship.

Review round 2 (Reviewer General on `677f879`): the closing-vs-missing
conflation [medium] is fixed here. `try_existing_worker` now returns a typed
`WorkerDispatch { Accepted, Missing, Closing }` instead of a bare
`Option<task>`, so the enqueue path branches explicitly: a task whose target
worker is draining for shutdown (`Closing`) records `DroppedOnShutdown` and
stops, rather than spawning a fresh worker that would clobber the closing
registry entry the shutdown barrier still counts. Two lightweight *behavioral*
unit proofs back the fix — they drive the real spawn/dispatch paths
synchronously (no relay host, no tmux), not leaf functions:
  - `async_worker.rs::terminal_outcome_receipt_routes_to_sender_worker_not_target`
    drives `complete_task_outcome` with a non-delivered outcome whose return-route
    runtime differs from the target's, and observes the receipt land on a worker
    registered at the SENDER's key while a worker at the target's key gets
    nothing — proving the receipt is built from `sender_return_route` and keyed to
    the sender, not the target.
  - `orchestration.rs::enqueue_drops_task_targeting_a_closing_worker` drives
    `enqueue_delivery_task` against a registered closing worker and observes the
    raced task resolve `DroppedOnShutdown` (surfaced as a receipt to the sender),
    the closing entry retained and fed nothing, no replacement spawned.
  The full cross-bundle end-to-end harness (`serve_connection` + real tmux across
  two runtimes) remains the Phase-2 follow-up above.

Review round 1 (Reviewer General on the first `c792e88`): the shutdown
accept-after-drain race [medium] is fixed here — a worker marks its registry
entry `closing` before draining so a late terminal-outcome receipt bounces
(best-effort drop) instead of landing in a receiver that is never polled, while
the entry stays counted for the shutdown barrier until the final unregister. The
two follow-up tests above remain.

## Implementation status (Phase 2 — Pty specialist slice)

Pty portion of task 2.3 (marker-line rendering for the receipt's pane-envelope
output) and task 5.1b (the matching transport README) landed on the `pty`
branch after RG review. The marker line `--- agentmux terminal-outcome
receipt ---` is emitted immediately before every receipt envelope in
`start_envelope_group`'s PTY-write loop, inside the same `writer.lock()` +
`write_all` block so the marker and the envelope are contiguous
(non-interleaved) under the writer lock. Detection is the typed
`DeliveryEnvelope.is_receipt` field the relay's terminal-resolution
chokepoint propagates from `AsyncDeliveryTask.is_receipt` via
`build_coder_envelope` — no Pty-side sender-identity inference (the earlier
`relay@RELAY` check was unsafe because `@RELAY` is the peer-relay principal
namespace and any peer with id `relay` would have been falsely flagged).
External test `pty_transport_start_envelope_group_emits_receipt_marker_for_receipt_only`
in `tests/unit/pty_transport.rs` drives `Delivery::start_envelope_group`
end-to-end against an in-memory writer and a minimal `PtyShared`, asserting
marker placement for receipts and absence for peers (no child process, no
timing, no `#[ignore]`). New `src/pty/README.md` documents the receipt
rendering, the Pty module layout, and the `--term-protocol` smoke-binary
flag from the recent add-pty-terminal-protocol-config work.

Deferred (not in the Pty slice):
- **2.3 Tmux marker line** — the same `RECEIPT_MARKER` line for tmux pane
  rendering. No Tmux-specialist lane exists currently; flag back to
  Coordinator/operator separately rather than silently dropping it.
- **5.1b Tmux README update** — the matching transport README for the Tmux
  receipt-rendering polish.
- **4.4 dedicated non-recursion test** — non-recursion is enforced
  structurally (the `is_receipt` gate is the first check at the single spawn
  site) and is exercised at runtime by the delivered receipt in the
  regression test; a standalone assertion is deferred because a
  *non-delivered* receipt cannot be constructed end-to-end without wedging
  the very sender pane the assertion reads.
- **Full cross-bundle receipt e2e (review follow-up)** — a real
  sender-bundle -> wedging-target-in-another-bundle test over
  `serve_connection` with a two-bundle catalog and two tmux runtimes.
- **§0 archive** — gated until all phases ship.

## 1. Spec deltas

- [x] 1.1 ADD `Asynchronous Terminal-Outcome Receipt`: relay-originated receipt
      envelope delivered to the sender through the sender's own transport, for
      non-delivered outcomes only, best-effort/drop-if-not-routable/no-persistence,
      non-recursive, and the `queued`-is-not-terminal-success contract.
- [x] 1.2 MODIFY `Async Delivery Observability`: widen the terminal-outcome
      inscription to `delivered` / `failed` (incl. `pane_wedged`) / `timeout` /
      `dropped_on_shutdown`, recorded regardless of whether a receipt is
      delivered.
- [x] 1.3 MODIFY `Tmux Prime Timeout`: remove the in-band
      "propagate that outcome to the MCP/CLI caller" clause; keep `Timeout`
      distinct and surface it (a non-delivered outcome) via the receipt plus the
      observability floor.
- [x] 1.4 Leave `Delivery Results Without ACK Protocol` unchanged (async-only
      submit; `queued` remains the accept-time response).

## 2. Terminal-outcome receipt

- [x] 2.1 Define the terminal-outcome receipt envelope/message type: a
      relay/system-originated envelope carrying the original `message_id`, the
      delivery target, the terminal outcome, and any `reason_code`, distinct from
      a peer `incoming_message`. It carries a marker (a boolean flag or a
      dedicated message variant — implementer's call) that the relay-side
      terminal-resolution point checks; transports stay receipt-agnostic.
- [x] 2.2 At the async worker's terminal-resolution point, for a non-delivered
      outcome (`failed` incl. `pane_wedged`, `timeout`, `dropped_on_shutdown`),
      enqueue a receipt addressed to the original sender and route it through the
      existing delivery pipeline to the sender's transport. Emit no receipt for a
      `delivered` outcome. Build the receipt's delivery task from the SENDER's
      bundle + runtime directory (the sender's home bundle for a cross-bundle
      send), NOT the target's — otherwise a cross-bundle receipt misroutes.
- [ ] 2.3 Render the receipt per transport: Tmux (pane), ACP (turn), Pty (pty
      write). The UI transport already emits the sender `delivery_outcome` frame
      via `emit_sender_delivery_outcome_event`; reconcile it as the UI rendering
      of the receipt. ACP-specific: the receipt must be a flush barrier (do not
      coalesce with peer traffic in the flush group), bypass quiescence
      (zero quiet-window; informational, no follow-on expected), and carry no
      choice-decider sessions. Tmux/Pty: render via the existing pane-envelope
      renderer plus a receipt marker line for human visibility.
- [x] 2.4 Enforce non-recursion at the single relay-side spawn site via the
      §2.1 marker: spawn a receipt only when the resolving delivery is itself not
      a receipt and the outcome is non-delivered; a receipt's own outcome records
      to `relay.log` and stops.
- [x] 2.5 Best-effort: drop the receipt when the sender session is not routable
      (its delivery worker/session does not exist; a wedged-but-registered sender
      is routable-but-slow and queues); no persistence, no retry. Drive the
      receipt from the same terminal resolution that unblocks the
      per-`(namespace, runtime_directory, target_session)` delivery queue (resolve
      once, fan out).

## 3. Observability floor

- [x] 3.1 Emit the terminal-outcome inscription for every terminal outcome
      (`delivered` / `failed` incl. `pane_wedged` / `timeout` /
      `dropped_on_shutdown`), independent of whether a receipt was delivered.

## 4. Tests

- [x] 4.1 REGRESSION (coder transport): a non-delivered outcome (e.g.
      `pane_wedged` or `timeout`) for a queued message is observable by the
      original coder-session sender via the receipt delivered to its transport —
      without reading `relay.log`.
- [x] 4.2 A `delivered` outcome produces no receipt but is recorded in
      `relay.log`.
- [x] 4.3 A non-delivered outcome for a sender that is not routable drops the
      receipt (no persistence/retry) yet is still recorded in `relay.log`.
- [ ] 4.4 A receipt does not itself produce a receipt (non-recursion).
- [x] 4.5 The receipt is keyed by the original `message_id` and names the delivery
      target and `reason_code`, so the sender can correlate it to its accept-time
      `queued` result; the receipt is relay/system-originated, not a peer message.

## 5. Documentation and validation

- [x] 5.1a Update `src/relay/README.md` for the terminal-outcome receipt, its
      relay-side spawn/route/drop mechanics, and the `queued`-is-not-success
      contract.
- [ ] 5.1b Update the transport READMEs for the receipt's per-transport
      rendering (deferred with 2.3 — the transport-rendering phase).
- [x] 5.2 Run `openspec validate deliver-async-terminal-outcomes --strict`.
- [x] 5.3 Run `cargo fmt --check`.
- [x] 5.4 Run `cargo clippy --all-targets --no-deps -- -D warnings`.
- [x] 5.5 Run `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml`.
