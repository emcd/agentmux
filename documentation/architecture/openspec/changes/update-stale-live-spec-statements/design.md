## Context

Six confirmed contradictions in the live specifications, plus one escaping
artifact found while repairing them. No architecture is being decided here; the
only real design question is how to make seven `## MODIFIED Requirements` deltas
without losing content.

## The one hazard worth designing around

A `## MODIFIED Requirements` delta replaces the **entire** named requirement at
sync. Every scenario not carried forward is deleted from the live spec, silently
and with no behavior change to signal it. `openspec validate --strict` cannot see
this: it checks that a delta is well formed, never that it is complete against
the requirement it replaces.

Seven requirements are modified here. Several are large — `ACP Look Snapshot
Contract` alone runs 158 lines with nine scenarios, and `Relay Configuration
File` runs 171. The intended edit in each case is between one line and one table
row. Retyping a 158-line requirement to change one bullet is a content-loss
event waiting to happen.

## Approach: seed verbatim, then edit

Each delta was produced by extracting the live requirement byte-for-byte into
the delta file, then applying only the intended edit on top:

1. `awk` the requirement out of the live spec from its `### Requirement:` header
   to the next one, into the delta file under a `## MODIFIED Requirements`
   heading.
2. Apply the single intended change as a targeted edit.
3. Confirm with `scripts/verify-openspec-deltas.py` that the drop set is empty.

This makes retention the default rather than something to remember. The result
is `0 errors, 0 dropped scenarios` across all seven, with the only additions
being two deliberate new scenarios.

The alternative — authoring each delta from the requirement's meaning — is what
produces dropped scenarios, and is how the one such drop found in
`add-e2e-test-harness` this week happened: a requirement rewritten to add two
scenarios, with a third silently not carried forward.

## Why `unreachable-dwell-ms` gets a table row rather than prose

`Relay Configuration File` already discusses the key by name ("`unreachable-dwell-ms`
is not an exception to this"), and asserts a zero-exclusion rule — "Every range
above excludes zero" — over a table that does not list it. Adding the row is what
makes both statements true. Default and range are taken from
`src/relay/configuration/delivery.rs`, and the governing text from the doc
comment above that constant, rather than being restated independently.

## Scope expansion, disclosed

`ACP Look Snapshot Contract` contains eleven literal `\'` sequences — an escaping
artifact committed at some point, rendering as `operator\'s` in the document.
They are not among the six defects.

All eleven are inside the requirement this change already rewrites, so correcting
them creates no inconsistency elsewhere in that file, and leaving them while
retyping the surrounding text would be perverse. They are corrected here and
called out so the reviewer's diff has no unexplained hunks. Nothing outside that
requirement is touched.

## Two edits that are not deltas

The `cli-surface` Purpose preamble and the `session-relay` hub preamble are prose
rather than requirements, so no delta mechanism governs them and direct edit is
the only available route. They are implementation tasks rather than spec deltas.

The `session-relay` edit is a second site of the same four-session-type error;
correcting only the `addressing-routing` requirement would leave the corpus half
right.

## Deliberately not here

- The per-capability allowed-scope cap contradiction, which removes a normative
  rule and a scenario rather than correcting a stale statement, and which gates
  the `raww` restructuring.
- Retiring `session-relay` as a capability, which would delete a whole capability
  spec — the first such removal in this project.

Both are tracked and would each change what the specifications *require*. This
change changes only what they *say*.
