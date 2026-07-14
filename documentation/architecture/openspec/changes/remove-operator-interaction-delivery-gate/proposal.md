# Change: Remove the operator-interaction delivery gate

## Why

A `send` to `reviewer@agentmux-aux` was accepted, acked as `queued`, and then
never delivered. It sat in the quiescence classifier for **40 minutes** and
resolved only when the relay was killed. Two review handoffs to the Reviewer
General — the merge gate — vanished this way. See `issues/relay/52`.

Nothing crashed. No task was lost. No forwarder died. The delivery was still
politely waiting, exactly as the spec instructs it to.

`openspec/specs/session-relay/spec.md:3340-3345` says:

> Active operator interaction indefinitely suppresses unresponsive
> classification until it clears; the prime timer SHALL NOT fire while
> operator interaction is active **regardless of how long the interaction
> persists**.

That sentence is the bug, written down as a `SHALL`. The implementation is
faithfully obeying it.

### The mechanism

`quiescence_classify_step` (`src/transports/quiescence.rs:412-423`) returns
`NeedsWait` when `operator_interaction_active` is set — and it returns
**before** the prime-timeout check at line 549. The timeout is therefore
unreachable for as long as the signal persists. No coder in practice sets
`prime-timeout-ms`, so `prime_deadline` is `None` and the wait falls back to
`unbounded_deadline()` (line 603) — literally `now + 1 year`. Setting
`prime-timeout-ms` would not rescue it either: the loop's own comment at lines
673-677 assumes "the prime-timeout branch fires on the next iteration," an
assumption the early return invalidates. It would hot-spin instead of blocking.
Neither path terminates.

### The trigger is a mouse wheel

`operator_interaction_active` (`src/tmux/pane.rs:66-78`) is true when tmux
reports `#{pane_in_mode} = 1` (copy-mode) or a client key-table other than
`root`. **A mouse-wheel scroll enters copy-mode.** So scrolling an agent's pane
to read it silently blocks every delivery to that agent, indefinitely, with no
diagnostic after the first tick.

It is undetectable from outside the relay: `capture-pane` keeps returning live
pane content while in copy-mode, so `agentmux look` shows an innocent idle pane
sitting at its prompt. The incident was misdiagnosed as a cross-namespace
routing failure for exactly this reason.

### Why the gate should be deleted rather than bounded

The gate exists to stop `tmux send-keys` from landing in copy-mode, where keys
are interpreted as copy-mode commands instead of reaching the child. That
concern was legitimate. It is also obsolete: message bodies have since moved to
`paste-buffer`, and `paste-buffer` writes **straight to the pane's pty**,
bypassing the copy-mode key table entirely.

Verified empirically against tmux 3.4 (full matrix and method in `design.md`):

| Mechanism | Reaches the child while in copy-mode? |
| --- | --- |
| `paste-buffer` (message body) | **Yes** — pane stays in copy-mode |
| `send-keys Enter` (submit) | **No** — swallowed by the copy-mode key table |
| `send-keys -H 0d` (raw CR byte) | **No** — still routed through the key table |
| `paste-buffer` carrying a bare CR | **Yes** — submits; pane stays in copy-mode |

Two further measurements make deletion safe rather than merely possible:

- `capture-pane` and `#{cursor_x}` both report the **live** pane, not the
  scrolled-back copy-mode view. A pane scrolled up 20 lines still reports
  `LIVE_PROMPT>` at cursor column 13, identical to the unscrolled capture. So
  prompt-readiness detection — and therefore wedge/unresponsive classification —
  is entirely unaffected by copy-mode. The classifier does not need protecting
  from it.
- Delivery into a copy-mode pane does not disturb the operator. The paste lands
  in the child; tmux does not auto-scroll on new output; the operator's scroll
  position is preserved.

So the gate protects a mechanism we no longer use, at the cost of an
unbounded silent hang on the merge-gate path. Per the project's alpha policy,
it is deleted outright rather than bounded or deprecated.

### The trailing-Enter landmine

`inject_literal_text` is only **half** converted. `src/tmux/pane.rs:225` still
submits with `send-keys -t <pane> Enter`, and that half **is** still swallowed
by copy-mode. Deleting the gate alone would therefore paste the message body
into the coder's prompt and leave it sitting there unsubmitted until the
operator's next keystroke — quietly worse than the hang it replaces. The submit
must move to `paste-buffer` in the same change.

## What Changes

- **Delete the operator-interaction branch** from `quiescence_classify_step`
  (`src/transports/quiescence.rs:412-423`), along with the
  `delivery_operator_interaction` diagnostic it emits. Classification proceeds
  on live pane content, which copy-mode does not affect.
- **Delete `WedgeObservation.operator_interaction_active`** and the `WedgeProbe`
  plumbing that populates it, including the tmux
  `operator_interaction_active` / `pane_in_mode_active` / `active_client_key_table`
  queries in `src/tmux/pane.rs` that exist only to feed it.
- **Move the submit off `send-keys`.** `inject_literal_text` sends the trailing
  Enter as its own **unbracketed** `paste-buffer` carrying a CR (`\r`) instead
  of `send-keys Enter`. The body keeps bracketed paste (`-p`) so multi-line
  content does not submit early — which is precisely why the Enter was a
  separate key in the first place. Both halves then bypass the copy-mode key
  table and the whole tmux injection path becomes copy-mode-transparent. The CR
  paste stays inside the existing `append_enter` guard, so it is emitted only
  when the write requests submission: a `raww` write with `no_enter=true`
  (`append_enter=false`) still injects the body and no submit, preserving the
  `Relay raww transport behavior` contract. The new `Copy-Mode-Transparent
  Injection` requirement pins this scoping explicitly.
- **Retire the `PendingChoiceProbe` canonical sequence.** Its entire purpose was
  to assert indefinite suppression under operator interaction. The five
  canonical probe sequences become four: unresponsive, wedged, slow-prompt, and
  normal-flow. A pane genuinely stuck on a choice dialog is wedge-class and is
  already covered by `AlwaysWedgeProbe` — the classification depends on pane
  content, not on whether an operator happens to be scrolling.
- **Update the specs.** The suppression is normative in two live specs; both are
  amended (see Impact).

The typing guard is unaffected and remains the correct mechanism for "do not
inject while the user is mid-keystroke": `input_idle_cursor_column`
(`session-relay/spec.md:425-435`) gates injection on the cursor sitting at the
configured idle column. That guard reads live pane state and is orthogonal to
copy-mode.

## Impact

- **Affected specs.** The operator-interaction gate is not a standalone
  requirement — it lives as clauses inside existing requirements — so every
  spec edit is a `MODIFIED` requirement (each reproduced verbatim from the
  post-`add-wedge-detection-busy-state` live spec, with only the gate clauses
  removed), plus one `ADDED` requirement. There are no `REMOVED` requirements.
  - `session-relay` — `Quiescence-Gated Delivery`, `Prompt-Readiness Template
    Gating`, `Tmux Prime Timeout`, and `Tmux Wedged State Detection` lose their
    operator-interaction preconditions; the `Prime timeout does not fire while
    operator interaction is active` and `Wedge does not fire while operator
    interaction is active` scenarios are removed. A new `Copy-Mode-Transparent
    Injection` requirement pins the paste-buffer submit so the gate cannot be
    reintroduced by a future change that reaches for `send-keys`.
  - `transport-abstraction` — `Three-State Delivery Classifier` (the
    operator-interaction step drops out of the Busy branch-ordering contract,
    which renumbers; the two "requires no pending operator interaction"
    scenarios are replaced by a copy-mode scenario) and
    `Transport-Internal Probe Seam for Testability` (loses the
    operator-interaction observation and the `pending-choice` canonical
    sequence — five sequences become four). The delta also repairs an orphaned
    bullet fragment the busy-state archive left dangling.
  - **Delta-base coupling:** both `Three-State Delivery Classifier` deltas are
    written against the post-busy-state spec, so this change MUST archive after
    `add-wedge-detection-busy-state` (now archived, master `882283b`). See
    `tasks.md` §0.
- **Affected code**
  - `src/transports/quiescence.rs` — remove the branch, the field, and the
    module-doc precedence text that describes them.
  - `src/tmux/pane.rs` — remove the operator-interaction queries; change the
    submit path in `inject_literal_text`.
  - `src/tmux/quiescence_probe.rs` — drop the trait method and its adapter use.
  - `src/pty/state.rs` — remove the two `operator_interaction_active: false`
    struct-literal lines (304, 460). **Owned by the Pty lane**; landed by Pty
    Specialist as a separate commit on this branch so the removal stays atomic
    (agreed shape, see `tasks.md`).
  - `tests/unit/tmux_transport.rs`, `tests/unit/transports_quiescence.rs` —
    drop the `ScriptedProbe` operator-interaction method, the struct-literal
    fields, and the `PendingChoiceProbe` sequence; add the regression tests
    below.
  - `tests/integration/relay_delivery_runtime.rs` — migrate the existing
    contracts the injection change invalidates: the two paste-buffer submit
    shape (`relay_delivery_sends_submit_in_separate_tmux_command`,
    `relay_raww_tmux_default_queues_and_appends_enter`), the removed-gate test
    (`relay_async_delivery_does_not_inject_while_pane_in_mode`, inverted into
    the copy-mode regression), and the preserved body-only path
    (`relay_raww_tmux_no_enter_omits_enter_command`). Fake-tmux fixture / log
    parsing may need to distinguish the CR buffer from the body buffer. See
    `tasks.md` §5.5.

## Non-goals

- **The silent-`queued` ack.** A delivery that ends `failed`, `wedged`, or
  `dropped_on_shutdown` still reports success-shaped `queued` to the sender and
  records the real outcome only in `relay.log`. That is a genuine defect and is
  filed separately as `issues/relay/53` — and it is worse than a missing
  feature: the live spec contradicts itself, requiring both async-only
  acceptance (`session-relay/spec.md:338-344`) and propagation of a distinct
  timeout result to the caller (`3347-3351`). Reconciling those two `SHALL`s is
  a design conversation, sequenced after this change.
- **Head-of-line blocking.** One worker per
  `(namespace, runtime_directory, target_session)` on an unbounded mpsc queue
  means a wedged head delivery stalls every later message to that session. This
  change removes the only known unbounded wedge, so the queue drains; the
  unbounded queue itself is left for `issues/relay/53`.
- **Retuning `WEDGE_CONSECUTIVE_TICKS` or the prime window.** Untouched.
- **A Pty operator-interaction analogue.** Pty has no copy-mode, key-table, or
  operator-attached TUI. Should one ever arrive, it would be a Pty-specific
  field with Pty-specific semantics, not a revival of this shared one.

## Validation plan

- `cargo nextest run --locked --config-file
  .auxiliary/configuration/nextest.toml` — no regressions across the remaining
  four canonical probe sequences.
- `cargo nextest run --locked --config-file
  .auxiliary/configuration/nextest.toml --features pty` — green after the Pty
  two-line deletion lands.
- New regression test: a probe reporting a **prompt-ready pane** must resolve
  `Delivered` on the first quiescent tick with no suppression branch available
  to park it. This is the unit-level statement of "a scrolled pane still gets
  its message."
- New tmux integration test: `inject_literal_text` against a pane placed in
  copy-mode (`tmux copy-mode -u`) delivers **both** the body and the submit —
  the child receives the full line — and the pane is **still in copy-mode**
  afterwards (`#{pane_in_mode}` remains `1`), proving the operator's scroll
  position survives delivery. This test fails against today's `send-keys Enter`
  and is the regression lock for the landmine.
- `cargo clippy --all-targets --no-deps -- -D warnings` and
  `--features pty` — silent.
- `cargo fmt --check` — silent.
- `openspec validate remove-operator-interaction-delivery-gate --strict` — valid.
- Manual: scroll a live agent's pane into copy-mode, send it a message, confirm
  the message arrives and the scroll position is undisturbed.
