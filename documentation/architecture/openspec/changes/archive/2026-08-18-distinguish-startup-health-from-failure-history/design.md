## Context

Two surfaces read from the same area and mean different things by it. The relay
computes `startup_health` by probing each configured session's transport when it
builds a list payload, and separately loads a persisted per-bundle history of
startup failures to populate `startup_failure_count` and
`recent_startup_failures`. The history is not consulted when deciding health.

The specification does not describe it that way. It defines `degraded` as "at
least one configured session is ready and at least one startup attempt failed",
which reads health off recorded attempts, and it describes the failure history's
shape, ordering, and bounded eviction without ever saying when a record leaves
for any other reason.

The gap is not academic. A failure record that never expires is indistinguishable
in the payload from a session failing right now, and a specification that only
bounds the history by count licenses exactly that. The condition surfaced as a
concrete defect in the relay's own clearing path — a session that failed,
recovered, and failed again kept the second record indefinitely — and the
specification could not be cited against it, because nothing in it says a record
answers a question that can stop being true.

## Goals / Non-Goals

**Goals:**

- State that bundle startup health is a function of current session readiness,
  matching what the relay computes and what an operator means by "healthy".
- Give startup-failure records a defined end: a record describes a startup
  attempt that has not since been superseded by that session serving.
- Make the non-coupling explicit, so health cannot be re-derived from the history
  by a later implementation reading the log to decide readiness.

**Non-Goals:**

- Changing relay behavior. Readiness-based health and per-session clearing are
  both implemented; this change makes them normative.
- Defining per-transport readiness. What "the transport reports it serving" means
  for tmux, ACP, Pty, UI, or pub/sub belongs to the transport contracts. The
  hardcoded readiness answers some transports currently return are a separate
  question and are deliberately untouched here.
- Restating either rule in `cli-surface` or `mcp-tool-surface`. Those specs
  describe payload shape.
- Revisiting the 256-record bound, the ordering guarantee, or persistence across
  restarts.

## Decisions

**Health is readiness, evaluated per payload.** The alternative is to keep health
derived from recorded attempts and instead constrain when attempts are forgotten,
which is what the current wording implies. Rejected: it makes health a function
of a log's retention policy, so the same running bundle reports differently
depending on how aggressively history is pruned. Readiness is also the only one
of the two that can go back up on its own — a session that recovers is serving
again, and no amount of reasoning about past attempts recovers that fact.

The two are not equivalent, and the difference is worth stating rather than
glossing: a configured session that is not ready and never recorded a failed
attempt makes the bundle degraded under the readiness rule and does not under the
attempt rule. Readiness is the answer that matches the question an operator is
asking.

**A record's lifetime is tied to an observation, not to a clock or a count.** The
alternatives were a time-to-live, or keeping only the most recent record per
session. A TTL invents a duration nothing else in the system uses and would
either expire a still-true failure or retain a superseded one, depending on how
it was tuned. Keeping only the newest record per session discards the sequence of
distinct causes across repeated attempts, which is the part of the history worth
reading when a session is failing for a different reason each time. Tying
expiry to the session serving keeps every record that still describes something
and drops exactly those that no longer do.

**Clearing is specified as applying on every observation, not merely on the
first.** This is the one place the delta says something an implementation could
otherwise reasonably not do — and the place the implementation actually got it
wrong, by treating "this session has been cleared once" as equivalent to "this
session's history is empty". A requirement that only says records clear on
recovery is satisfiable by a single clear per session per process, so the
scenario for the second cycle carries the weight here.

**The history keeps a `bundle_name` field the persisted record does not carry.**
Left as-is: it predates this change, the emitted event does carry it, and
correcting it means deciding whether the field belongs to the record or to the
event. Noted rather than folded in.

## Risks / Trade-offs

- **The degraded condition genuinely changes meaning, not just wording.** A
  bundle with a configured session that never attempted startup now reports
  degraded where a literal reading of the old text did not → This matches the
  relay's existing behavior, so nothing observable changes; the risk is confined
  to a reader who trusted the old text over the implementation.
- **"Observed serving successfully" names two triggers, which couples the spec
  loosely to how the relay observes.** → The triggers are the observable events
  an operator can reason about (a session started, a message reached it) rather
  than internal call sites, and leaving the phrase open would make the
  requirement untestable.
- **Specifying non-coupling is a negative requirement, which cannot be
  demonstrated by a passing scenario.** → It is stated so a reviewer can cite it,
  and the recovered-session scenario gives it teeth from one direction: a bundle
  with a recorded failure and every session ready must report healthy.

## Open Questions

None.
