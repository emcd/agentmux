## Context

The Tmux readiness probe evaluates a configured prompt regex against the
inspected pane tail and, when configured, checks the idle cursor column. The
OpenCode pane layout has a third observable condition: the input box can hold
typed text while the frame below it still matches the idle prompt regex. The
implementation therefore performs a private second predicate after regex
success and before the cursor check.

The existing transport contract predates this predicate and describes regex
and cursor matching as sufficient. It also describes template matching as a
pre-injection condition for every transport, although Pty writes envelopes
before its readiness wait. This change makes both parts of the contract agree
with the implementation without adding configuration or a public API.

## Goals / Non-Goals

**Goals:**

- Specify the OpenCode frame suffix that identifies the layout to which the
  second predicate applies.
- Specify the exactly-three-row input region and the 99/100 whitespace
  boundary used by the implementation.
- Specify that compose text is a readiness mismatch, not a terminal failure,
  and that non-OpenCode or malformed frames retain normal template semantics.
- State the transport commitment boundary: Tmux readiness gates injection,
  while Pty readiness resolves outcomes after bytes have been written.
- Keep the implementation private and prove the contract through the
  production-path test.

**Non-Goals:**

- Add a coder configuration field or change prompt-template serialization.
- Generalize the compose predicate to ACP, Pty, or other Tmux layouts.
- Infer whether a coder is thinking, hung, or waiting on an operator.
- Preserve layouts beyond the measured three-row OpenCode 1.18.9 shape.

## Decisions

### Gate on the adjacent frame suffix

The helper recognizes an info row with optional leading whitespace followed
immediately by a separator line whose trimmed content is `╹` plus only 20 or
more `▀` characters, then a whitespace-prefixed status row containing
`ctrl+p commands`. When multiple valid suffixes occur, it selects the
bottommost one. This is preferred over independent `contains` checks because
unrelated chat history or status text must not activate the OpenCode-specific
predicate.

### Keep the predicate internal

The readiness template remains unchanged. The frame suffix is sufficient to
scope the behavior without exposing another configuration surface that could
be misapplied to other coders or layouts. The matcher and production helper
remain private; the single inline test is permitted because it exercises a
crate-private production path with no public seam.

### Treat compose text as a Tmux readiness mismatch

Compose text sets readiness to false while retaining `regex_matched = true`
and a diagnostic reason. Tmux does not interpret this mismatch as failure;
the existing readiness bound and other terminal conditions remain responsible
for ending the wait. A non-OpenCode matcher that succeeds bypasses the
OpenCode predicate and proceeds to its configured cursor check.

## Risks / Trade-offs

- [Layout sensitivity] The predicate models exactly three input rows and the
  99/100-space sidebar boundary measured in OpenCode 1.18.9 → keep the
  constraint in the contract and subsystem README, and require new captures
  before changing the implementation.
- [False frame recognition] A future UI could reuse the same suffix while
  changing its input layout → the production test and contract boundary make
  the assumption visible, but a future layout update must revise both.

## Migration Plan

No configuration or persisted-data migration is required. The change adds no
new key and changes no public API. If the contract or layout later changes,
update the delta/specification, preserved captures, implementation, and
production-path tests together.

## Open Questions

None.
