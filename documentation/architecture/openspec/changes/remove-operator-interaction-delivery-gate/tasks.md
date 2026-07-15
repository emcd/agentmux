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
- [ ] 0.2 Downstream archive coupling: the active `add-pty-transport` proposal
      still carries ~12 `operator_interaction_active` references (its
      `design.md`, `proposal.md`, `tasks.md`, and both spec deltas) that would
      RE-ADD the field at its own archive. Owned by the Pty lane (agreed option
      1): Pty updates that proposal against the post-archive live spec, sequenced
      AFTER this change archives. Not a blocker for merging this change — only a
      constraint on archive order between the two.

## 1. Tmux injection path (`src/tmux/pane.rs`)

- [x] 1.1 Change `inject_literal_text` to submit via an **unbracketed**
      `paste-buffer` carrying `\r` instead of `send-keys -t <pane> Enter`.
      The body keeps `paste-buffer -d -p` (bracketed) so multi-line content
      does not submit early. Both paste sites now go through a shared
      `paste_into_pane` helper with a `PasteMode` flag; the CR paste stays
      inside the `append_enter` guard so `no_enter=true` injects no submit.
- [x] 1.2 Delete `operator_interaction_active`, `pane_in_mode_active`, and
      `active_client_key_table`. They exist only to feed the gate.
- [x] 1.3 Confirmed no other caller depends on those three functions (only
      `quiescence_probe.rs` referenced `pane::operator_interaction_active`, now
      removed with §3.1).

## 2. Classifier (`src/transports/quiescence.rs`)

- [x] 2.1 Deleted the operator-interaction branch in `quiescence_classify_step`
      and the `delivery_operator_interaction` diagnostic.
- [x] 2.2 Deleted `WedgeObservation.operator_interaction_active` (and its doc).
- [x] 2.3 Updated the module-level docs: the branch-ordering list renumbers
      (1-5), the "Operator interaction ... indefinitely suppresses" paragraph is
      removed, the `running`-branch comment's stale-guard reference is corrected,
      and the prime-timeout comment's operator-interaction sentence is dropped.
- [x] 2.4 Confirmed `unbounded_deadline()` retains its legitimate caller (the
      `prime_deadline = None` fallback `NeedsWait`). No deliverable pane can park
      forever: a prompt-ready pane resolves `Delivered`, and a wedge-class pane
      fires `Wedged` via the counter even with no prime deadline. The only
      remaining unbounded park is the pre-existing `prime_timeout_ms = None` +
      genuinely-unresponsive-empty-pane case — an explicit opt-in, unchanged by
      this work, and NOT the deliverable-pane hang the gate produced.

## 3. Tmux probe (`src/tmux/quiescence_probe.rs`)

- [x] 3.1 Removed the `operator_interaction_active` trait method, its
      `RealPaneQuiescenceProbe` implementation, and the now-unused
      `pane::operator_interaction_active` import; updated the trait doc (five
      probe classes → four).
- [x] 3.2 Removed the field population in the `TmuxAsWedgeProbe` adapter and
      corrected the adapter doc-comment's `observe()` call description.

## 4. Pty (`src/pty/state.rs`)

- [x] 4.1 Deleted the two `operator_interaction_active: false` struct-literal
      lines. Reassigned from the Pty lane to Backend by operator direction: the
      relay branch is checked out in Backend's worktree, so it cannot also be
      checked out in Pty's worktree to land a commit there. Rather than stack a
      break-then-fix pair, this deletion is **folded into the implementation
      commit** so no commit on the branch ever removes the shared field while
      `src/pty/state.rs` still sets it — the `--features pty` build is atomic at
      every commit. Pty and RG review the result.

## 5. Tests

- [x] 5.1 `tests/unit/transports_quiescence.rs` — dropped the
      `operator_interaction_active` struct-literal fields. (No suppression-
      behavior tests lived in this file; all its tests exercise the Busy
      branch, which is unaffected.)
- [x] 5.2 `tests/unit/tmux_transport.rs` — dropped the `ScriptedProbe`
      `operator_interaction_active` method, the `ProbeObservation`
      `operator_interaction` field + `with_op_interaction` builder, and the
      `pending_choice_probe_neither_timeout_nor_wedge` test; module doc now
      lists four probe classes.
- [x] 5.3 NEW regression (unit):
      `prompt_ready_resolves_delivered_under_unbounded_prime` — a prompt-ready
      probe resolves `Delivered` on the first quiescent tick under
      `prime_timeout_ms = None`, with no suppression branch available to park
      it. The unit-level statement of "a scrolled pane still gets its message."
- [x] 5.4 NEW regression (real-tmux integration):
      `relay_raww_submits_through_copy_mode_pane` in
      `tests/integration/relay_delivery_async.rs`. Puts a real tmux pane into
      copy-mode, dispatches a `Raww` through the public relay path to a child
      that echoes each submitted line wrapped in `ECHOED[...]`, then asserts the
      wrapper appears (so the body AND the carriage return both crossed
      copy-mode and completed the child's `read`) and `#{pane_in_mode}` still
      reports `1` (the operator's scroll position survives). Built on the
      existing real-tmux harness in `tests/integration/support/relay_delivery.rs`
      (`tmux_available`, `TmuxServerGuard`, `spawn_session`,
      `wait_for_pane_contains`) — no inline seam; skips gracefully when tmux is
      absent. Confirmed it FAILS against a `send-keys Enter` submit (the pasted
      body arrives but the wrapper never does, because copy-mode swallows the
      keypress) and passes on the CR paste. This is the automated lock for the
      unique behavioral claim.

      (Correction: an earlier draft of this task claimed no real-tmux harness
      existed and proposed substituting command-shape plus manual coverage. That
      premise was wrong — the harness above is already used by
      `relay_delivery_async`, `relay_delivery_prompt`, and `session_relay_look`.
      The §5.5 fake-tmux contracts still independently lock the command shape;
      §6.7 remains as end-to-end manual confirmation.)
- [x] 5.5 `tests/integration/relay_delivery_runtime.rs` — migrated the existing
      contracts the injection change invalidates. Two shared predicates
      (`is_body_paste_line` = ` paste-buffer ` + `-t %1` + ` -p `;
      `is_submit_paste_line` = same but WITHOUT ` -p `) distinguish the
      bracketed body paste from the unbracketed CR submit in the command log;
      no fixture change was needed (the CR payload lands in its own
      `<log>.buffer.<name>` file, read via the existing
      `read_paste_buffer_content`).
      - `relay_delivery_sends_submit_in_separate_tmux_command`: now asserts one
        bracketed body paste, a separate unbracketed CR paste (content `\r`)
        ordered after the body, and no `send-keys`.
      - `relay_async_delivery_does_not_inject_while_pane_in_mode` → renamed
        `relay_async_delivery_injects_even_while_pane_in_mode`: inverted — with
        `#{pane_in_mode}=1` the delivery now reaches body + submit; asserts no
        `send-keys`. This is the command-shape copy-mode-transparency lock.
      - `relay_raww_tmux_default_queues_and_appends_enter`: now asserts the body
        paste carries the literal text, a separate unbracketed CR paste exists,
        and no `send-keys`.
      - `relay_raww_tmux_no_enter_omits_enter_command`: asserts the body paste
        carries the literal text and NO unbracketed CR submit paste exists — the
        regression lock for the `Copy-Mode-Transparent Injection` no_enter
        carve-out.

## 6. Validation

- [x] 6.1 `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml` — green (689/689).
- [x] 6.2 `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml --features pty` — green (718/718).
- [x] 6.3 `cargo clippy --all-targets --no-deps -- -D warnings` — silent.
- [x] 6.4 `cargo clippy --all-targets --features pty --no-deps -- -D warnings`
      — silent.
- [x] 6.5 `cargo fmt --check` — silent.
- [x] 6.6 `openspec validate remove-operator-interaction-delivery-gate --strict`
      — valid.
- [ ] 6.7 Manual live validation (operator-scheduled): scroll a live agent's
      pane into copy-mode, send it a message, confirm the message arrives, is
      submitted, and the operator's scroll position is undisturbed. Can be run
      in the same sitting as `add-wedge-detection-busy-state` §6.7.

## 7. Documentation

- [x] 7.1 Scanned `src/transports/README.md` and `src/tmux/README.md` (and the
      broader docs tree) for operator-interaction-gate references: none present
      outside the OpenSpec specs (rewritten by this change's deltas at archive
      time) and the `add-pty-transport` proposal (Pty-owned, §0.2). No README
      edits required.
