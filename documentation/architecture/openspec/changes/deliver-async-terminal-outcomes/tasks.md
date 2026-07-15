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

## 1. Spec deltas

- [ ] 1.1 ADD `Asynchronous Terminal-Outcome Receipt`: relay-originated receipt
      envelope delivered to the sender through the sender's own transport, for
      non-delivered outcomes only, best-effort/drop-if-not-routable/no-persistence,
      non-recursive, and the `queued`-is-not-terminal-success contract.
- [ ] 1.2 MODIFY `Async Delivery Observability`: widen the terminal-outcome
      inscription to `delivered` / `failed` (incl. `pane_wedged`) / `timeout` /
      `dropped_on_shutdown`, recorded regardless of whether a receipt is
      delivered.
- [ ] 1.3 MODIFY `Tmux Prime Timeout`: remove the in-band
      "propagate that outcome to the MCP/CLI caller" clause; keep `Timeout`
      distinct and surface it (a non-delivered outcome) via the receipt plus the
      observability floor.
- [ ] 1.4 Leave `Delivery Results Without ACK Protocol` unchanged (async-only
      submit; `queued` remains the accept-time response).

## 2. Terminal-outcome receipt

- [ ] 2.1 Define the terminal-outcome receipt envelope/message type: a
      relay/system-originated envelope carrying the original `message_id`, the
      delivery target, the terminal outcome, and any `reason_code`, distinct from
      a peer `incoming_message`. It carries a marker (a boolean flag or a
      dedicated message variant — implementer's call) that the relay-side
      terminal-resolution point checks; transports stay receipt-agnostic.
- [ ] 2.2 At the async worker's terminal-resolution point, for a non-delivered
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
- [ ] 2.4 Enforce non-recursion at the single relay-side spawn site via the
      §2.1 marker: spawn a receipt only when the resolving delivery is itself not
      a receipt and the outcome is non-delivered; a receipt's own outcome records
      to `relay.log` and stops.
- [ ] 2.5 Best-effort: drop the receipt when the sender session is not routable
      (its delivery worker/session does not exist; a wedged-but-registered sender
      is routable-but-slow and queues); no persistence, no retry. Drive the
      receipt from the same terminal resolution that unblocks the
      per-`(namespace, runtime_directory, target_session)` delivery queue (resolve
      once, fan out).

## 3. Observability floor

- [ ] 3.1 Emit the terminal-outcome inscription for every terminal outcome
      (`delivered` / `failed` incl. `pane_wedged` / `timeout` /
      `dropped_on_shutdown`), independent of whether a receipt was delivered.

## 4. Tests

- [ ] 4.1 REGRESSION (coder transport): a non-delivered outcome (e.g.
      `pane_wedged` or `timeout`) for a queued message is observable by the
      original coder-session sender via the receipt delivered to its transport —
      without reading `relay.log`.
- [ ] 4.2 A `delivered` outcome produces no receipt but is recorded in
      `relay.log`.
- [ ] 4.3 A non-delivered outcome for a sender that is not routable drops the
      receipt (no persistence/retry) yet is still recorded in `relay.log`.
- [ ] 4.4 A receipt does not itself produce a receipt (non-recursion).
- [ ] 4.5 The receipt is keyed by the original `message_id` and names the delivery
      target and `reason_code`, so the sender can correlate it to its accept-time
      `queued` result; the receipt is relay/system-originated, not a peer message.

## 5. Documentation and validation

- [ ] 5.1 Update `src/relay/README.md` and the transport READMEs for the
      terminal-outcome receipt, its per-transport rendering, and the
      `queued`-is-not-success contract.
- [ ] 5.2 Run `openspec validate deliver-async-terminal-outcomes --strict`.
- [ ] 5.3 Run `cargo fmt --check`.
- [ ] 5.4 Run `cargo clippy --all-targets --no-deps -- -D warnings`.
- [ ] 5.5 Run `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml`.
