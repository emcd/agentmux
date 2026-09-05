## Context

The mapping this change specifies was written once, in the change archived as
`2026-04-17-update-list-to-list-sessions-and-bundle-state-fanout-mvp`, inside a
requirement named `Relay List Bundle Live-State Payload`. That requirement never
reached any live spec, and the archive-readiness gate in
`scripts/verify-openspec-deltas.py` found it by replaying the sync question
against every archive commit.

The rest of that requirement has been superseded several times over — a singular
`bundle` object where the corpus now specifies a `bundles[]` aggregate,
`sessions[]` where it now specifies `principals[]`, and an explicit "SHALL NOT
accept all-bundle list selectors" that `mcp-tool-surface` MCP List Sessions
All-Mode Aggregation directly reverses. Restoring it whole would introduce
contradictions rather than repair a gap.

Only the down-reason mapping survived in code without reaching the corpus. Two
independent surfaces implement it identically today.

## Goals / Non-Goals

**Goals:**

- Bind the two client surfaces to one statement of a value their callers branch
  on, rather than to each other's source.
- Keep the client-synthesized codes readable as a set distinct from the
  relay-reported ones.

**Non-Goals:**

- Restoring `Relay List Bundle Live-State Payload`, in whole or in a modernized
  form. It is superseded; this change deliberately rescues one fragment and
  leaves the rest lost.
- Changing behavior. Both code paths already agree; nothing needs to move.
- Reconciling the two reason vocabularies into one namespace. They describe
  different subjects and never co-occur.

## Decisions

**The requirement lands in `addressing-routing` rather than in each surface
capability.** `cli-surface` and `mcp-tool-surface` each carry a near-parallel
unreachable-relay fallback requirement, so stating the mapping in both would
create the duplicated inventory this corpus has been paying down — and would put
the drift the requirement exists to prevent inside the requirement itself.
`addressing-routing` already owns canonical list-payload semantics across
surfaces (its `hosted` and `ready` field rules constrain what CLI and MCP emit),
which makes it the existing home for a rule binding both.

Considered and rejected: `bundle-lifecycle`, which owns Bundle Down Reason
Precedence and so looks like the natural neighbour. It is the wrong home
precisely because it is adjacent — that requirement scopes itself to what the
relay reports, and hanging a client-side rule beside it invites a future reader
to fold the two code sets into one precedence list. Keeping them in separate
capabilities makes the disjointness structural rather than a sentence someone
must notice.

**The derivation is fixed at socket-path presence, and clients are forbidden
from refining it.** The value is only useful if every surface reaches the same
verdict from the same filesystem state; a client that probed harder to
distinguish a starting relay from a wedged one would report a different code for
a state its sibling reports differently, which is worse than the coarse answer.
The requirement therefore constrains the evidence, not just the outcome.

**Disjointness is stated as a property, not a precedence.** Saying "no payload
can carry both" answers the precedence question by dissolving it, which is
cheaper to keep true than an ordering rule between two sets that never meet.

## Risks / Trade-offs

**The codes are coarse and may want refinement later** → A future change that
distinguishes, say, a relay mid-startup from a wedged one has to amend this
requirement rather than add a code at one surface. That friction is intended:
the alternative failure — two surfaces disagreeing about the same socket — is
the one this change exists to prevent.

**A rule about client behavior lives in a capability whose other requirements
are mostly relay-side** → Mitigated by the requirement naming both fallback
requirements it serves, so a reader arriving from either surface can find it.
