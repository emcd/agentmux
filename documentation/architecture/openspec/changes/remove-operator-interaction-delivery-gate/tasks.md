# Tasks: Remove the operator-interaction delivery gate

## 0. Sequencing gate (READ FIRST)

This change MUST archive **after** `add-wedge-detection-busy-state`.

Both changes `MODIFY` the `Three-State Delivery Classifier` requirement in
`transport-abstraction`, and a `MODIFIED` delta replaces the requirement whole.
`add-wedge-detection-busy-state` is now archived (master `882283b`; its §6.7
manual validation was waived and re-filed as a standalone 0.9.0 todo), so the
live spec already carries the `Busy` pre-classification. The deltas here are
reproduced verbatim from that post-busy-state live spec with only the
operator-interaction gate clauses removed, so archiving this change replaces the
requirement cleanly without dropping the Busy text.

The failure mode this ordering avoids: if this change had been authored against
a pre-busy-state spec (no `Busy`) and archived after busy-state, the
whole-requirement replacement would **delete the Busy pre-classification**. That
is why the delta base matters and why the ordering is fixed, not incidental.

- [x] 0.1 `add-wedge-detection-busy-state` archived (882283b). Deltas rebased
      onto the post-busy-state live spec (`ffd2593`) and each MODIFIED
      requirement diff-verified to change only the gate clauses. Nothing further
      required before this change's own archive, which stays sequenced after
      882283b (already true — 882283b is an ancestor of this branch).

## 1. Tmux injection path (`src/tmux/pane.rs`)

- [ ] 1.1 Change `inject_literal_text` to submit via an **unbracketed**
      `paste-buffer` carrying `\r` instead of `send-keys -t <pane> Enter`
      (line 225). The body keeps `paste-buffer -d -p` (bracketed) so multi-line
      content does not submit early.
- [ ] 1.2 Delete `operator_interaction_active`, `pane_in_mode_active`, and
      `active_client_key_table` (lines 66-145). They exist only to feed the gate.
- [ ] 1.3 Confirm no other caller depends on those three functions.

## 2. Classifier (`src/transports/quiescence.rs`)

- [ ] 2.1 Delete the operator-interaction branch in `quiescence_classify_step`
      (lines 412-423) and the `delivery_operator_interaction` diagnostic.
- [ ] 2.2 Delete `WedgeObservation.operator_interaction_active` (line 158).
- [ ] 2.3 Update the module-level docs: the branch-ordering list (lines 34-42)
      renumbers, and the "Operator interaction ... indefinitely suppresses"
      paragraph (lines 50-52) is removed. The `running` branch comment at
      lines 477-479 references the now-absent guard and must be corrected.
- [ ] 2.4 Confirm `unbounded_deadline()` retains a legitimate caller. With the
      gate gone its remaining use is the `prime_timeout_ms = None` path, which
      is a documented opt-in-to-unbounded — keep it, but verify no path can now
      park forever without a terminal classification available to it.

## 3. Tmux probe (`src/tmux/quiescence_probe.rs`)

- [ ] 3.1 Remove the `operator_interaction_active` trait method (line 93) and
      its `RealPaneQuiescenceProbe` implementation (lines 162-165).
- [ ] 3.2 Remove the field population in the `WedgeProbe` adapter
      (lines 271, 291).

## 4. Pty (`src/pty/state.rs`) — OWNED BY THE PTY LANE

- [ ] 4.1 Pty Specialist deletes the two `operator_interaction_active: false`
      struct-literal lines (304, 460) as a separate commit **on this branch**,
      so the shared-field removal and its Pty cleanup merge atomically and no
      intermediate commit breaks the `--features pty` build. Agreed shape;
      Backend does not touch `src/pty/**`.

## 5. Tests

- [ ] 5.1 `tests/unit/transports_quiescence.rs` — drop the
      `operator_interaction_active` struct-literal fields and the
      suppression-behavior tests.
- [ ] 5.2 `tests/unit/tmux_transport.rs` — drop the `ScriptedProbe`
      `operator_interaction_active` method and the `PendingChoiceProbe`
      canonical sequence; the five canonical sequences become four.
- [ ] 5.3 NEW regression (unit): a probe reporting a prompt-ready pane resolves
      `Delivered` on the first quiescent tick, with no suppression branch
      available to park it. The unit-level statement of "a scrolled pane still
      gets its message."
- [ ] 5.4 NEW regression (tmux integration): `inject_literal_text` against a
      pane placed in copy-mode (`tmux copy-mode -u`) delivers **both** the body
      and the submit — the child receives the complete line — and
      `#{pane_in_mode}` still reports `1` afterwards, proving the operator's
      scroll position survives delivery. **This test must fail against today's
      `send-keys Enter`**; confirm it does before implementing 1.1, or it is not
      actually locking the landmine.
- [ ] 5.5 `tests/integration/relay_delivery_runtime.rs` — migrate the existing
      contracts the injection change invalidates (do NOT rely on full-suite
      failures to discover them):
      - `relay_delivery_sends_submit_in_separate_tmux_command` (fn at :885):
        asserts one body paste + `send-keys Enter`; update to two paste buffers
        and distinguish the bracketed body from the unbracketed CR.
      - `relay_async_delivery_does_not_inject_while_pane_in_mode` (fn at :1042):
        asserts the removed gate; replace/invert it — a pane in copy-mode now
        DOES receive delivery. Fold into the 5.4 copy-mode regression rather than
        duplicate.
      - `relay_raww_tmux_default_queues_and_appends_enter` (fn at :1101):
        asserts `send-keys Enter`; update to verify the unbracketed CR paste.
      - `relay_raww_tmux_no_enter_omits_enter_command` (fn at :1192): keep it
        proving body-only behavior under the revised mechanism (no CR paste when
        `no_enter=true`) — this is the regression lock for the
        `Copy-Mode-Transparent Injection` no_enter carve-out.
      Check whether the fake-tmux fixture / command-log parsing needs adjustment
      to identify the second buffer's CR payload distinctly from the body paste.

## 6. Validation

- [ ] 6.1 `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml` — green.
- [ ] 6.2 `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml --features pty` — green (after 4.1).
- [ ] 6.3 `cargo clippy --all-targets --no-deps -- -D warnings` — silent.
- [ ] 6.4 `cargo clippy --all-targets --features pty --no-deps -- -D warnings`
      — silent.
- [ ] 6.5 `cargo fmt --check` — silent.
- [ ] 6.6 `openspec validate remove-operator-interaction-delivery-gate --strict`
      — valid.
- [ ] 6.7 Manual live validation (operator-scheduled): scroll a live agent's
      pane into copy-mode, send it a message, confirm the message arrives, is
      submitted, and the operator's scroll position is undisturbed. Can be run
      in the same sitting as `add-wedge-detection-busy-state` §6.7.

## 7. Documentation

- [ ] 7.1 If any subsystem README describes the operator-interaction gate,
      update it in the same batch (`src/transports/README.md`,
      `src/tmux/README.md`).
