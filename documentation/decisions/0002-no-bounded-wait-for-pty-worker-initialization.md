# 0002. No bounded wait for Pty worker initialization

- Date: 2026-08-30
- Status: accepted
- Supersedes: —
- Superseded by: —
- Specs: transport-abstraction / Pty Transport Implementation

## Decision

Pty startup does not wait for worker initialization, and its return value
carries no answer about whether the target is ready. Readiness is answered
separately, by whatever predicate the transport contract exposes.

A worker that never arrives is therefore treated as a target that is never
ready, rather than as a startup failure.

## What we rejected, and why

A wait inside startup. It could not be made safe in either direction.

**Unbounded** — it never reached a verdict.

**Bounded** — its cleanup joined the worker thread. So a startup that timed out
*because that thread had stalled* then blocked on the same stall. The bound
relocated the hang rather than ending it.

The obstacle underneath both is that in-process terminal construction cannot be
interrupted. No cleanup can promise a cessation it has not observed, and a bound
that cannot be made true is worse than none.

## What this commits us to

Initialization failure has to reach the caller as unreachability rather than as
a startup error, because startup has already returned by the time it happens.
That is a consequence of the decision, not an independent choice.

## Related

The same reasoning governs the generation fence, where cessation is *observed*
on a bounded budget rather than joined. See
[0003](0003-remove-is-ready-rather-than-redefine-it.md) for the adjacent
decision about what readiness predicate the contract exposes.

Current mechanics — what startup spawns, which predicate reports readiness, how
an unresolved member is bounded, and how unreachability is surfaced — live in
the linked spec and in `src/pty/README.md`. They are deliberately not restated
here.
