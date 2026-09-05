## Context

These statements were found while checking a suspected defect that turned out
not to be one. The suspicion was that the `@GLOBAL` look-target rule lived in
the wrong capability — stated inside `raww` and nowhere a reader of the look
specs would find it. Reading it out showed the rule is stated three times over
in the right places: `addressing-routing` Suffix-Based Target Routing names Look
explicitly, `relay-routing-layer` Routing Resolution Stage owns the shared
stage, and `transport-contracts` Transport Capability Contract carries the
per-verb outcome with its own scenario. The `raww` sentence is a redundant
restatement, not the sole authority.

What the read did surface is that Relay Look Operation was never updated when
that shared stage landed. It still describes bare-target resolution against the
bound bundle, and a `bundle_name` request field, both of which the corpus and
the code have since moved past.

## Goals / Non-Goals

**Goals:**

- Make Relay Look Operation describe the operation the relay actually provides.
- Leave the corpus with one statement per rule rather than one per verb.

**Non-Goals:**

- Changing behavior. Every replacement scenario asserts what the code already
  does.
- Relocating the `@GLOBAL` rule out of `raww`. The redundancy is mild and the
  sentence binds both verbs correctly; splitting it would risk the two halves
  drifting, which is what the sentence exists to prevent.
- Reconciling `@<bundle>` and `@<namespace>` wording. `cli-surface`,
  `look-and-stream-events` and `mcp-tool-surface` agree on `@<bundle>` for look
  and `raww` uses `@<namespace>`; that asymmetry tracks a real difference in
  which namespaces each verb usefully addresses.

## Decisions

**State the relay-wide arm in Relay Look Operation rather than only citing the
capability that owns it.** The alternative — a bare cross-reference to Transport
Capability Contract — is cheaper to keep true but does not fix the actual
failure mode. A reader arriving at Relay Look Operation today infers from
"Reject unknown peer bundle" that every non-bundle suffix is an unknown bundle.
Correcting an inference that specific requires saying what happens instead, so
the requirement names both outcomes and cites the capability for the flag that
selects between them.

**Withdraw the bare-target allowance rather than reconcile it.** It is tempting
to read the allowance as describing the client-side convenience, since MCP and
CLI both qualify a bare target before the relay call. But the requirement opens
by scoping itself to "a relay-level read-only inspection operation", and its
scenario has the *relay* doing the resolving. There is no reading under which it
is describing the surfaces.

**Fix the `bundle_name` response mention as part of this change.** It is a
different field in a different direction from the `bundle_name` request field,
and it could have been left for a later pass. Leaving it would mean a reader
finding one requirement asserting a response field that its sibling three
requirements below declares retired — an internal contradiction inside a single
capability is worse than a stale claim, because neither statement can be
trusted once they are seen together.

**Replace the raww narration rather than delete it.** The paragraph contains a
real rule (the shared stage resolves `@GLOBAL` for both verbs) wrapped in change
narration. Deleting the paragraph outright would drop the only sentence stating
that the resolution is shared *for `@GLOBAL` specifically*; the surrounding text
establishes only that the stage is shared and that reserved-namespace rejection
is uniform.

## Risks / Trade-offs

**A documentation-only change to a scenario is invisible to CI** → Nothing fails
if a replacement scenario is wrong, since no test reads these. Mitigated by
citing the file and line of the implementing code for each claim in the
proposal, so a reviewer can check each one against a specific branch rather than
against a general impression.

**Stating the relay-wide arm in a second place** → Transport Capability Contract
owns the `can_be_looked` rule; this requirement now names its outcome too. The
duplication is bounded to naming two error codes and is cited back to its owner,
which is the trade accepted for correcting a wrong inference at the place the
reader forms it.
