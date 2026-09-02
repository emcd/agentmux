## Context

The specification claims a mechanism the code does not have and explicitly
guards against having. The repair direction is therefore fixed: the
specification moves to the code, not the reverse. That settles what would
otherwise be the only interesting question here, and leaves three smaller ones.

## Why deletion rather than restatement

The obvious repair is to replace the cap paragraph with a paragraph saying
there is no cap. That would be wrong twice over.

The rule already exists. `Authorization Control Vocabulary` states that the
policies file is authoritative and that every control accepts the full ladder
at parse time. Restating it inside `Uniform Cross-Bundle Authorization Model`
would duplicate a normative claim across two requirements in one capability,
which is how the two came to disagree in the first place — the cap paragraph is
itself a divergent restatement of what the vocabulary requirement already
governed.

The rule also is not what that paragraph was for. The requirement's preceding
paragraph already says reach is determined solely by the requester's configured
scope against the uniform threshold, with no per-operation policy in code.
Delete the cap paragraph and the requirement still says everything it needs to.

The same reasoning applies in `relay-routing-layer`, with one difference: there
the cap sentence and the operative rule share a sentence. The operative half —
the relay applies no per-operation cross-namespace logic in handler or routing
code — is kept, and only the clause naming a schema allowed-scope set as the
authority is removed.

A negative assertion ("there SHALL be no per-capability cap") was considered and
rejected. It buys nothing the vocabulary requirement does not already provide,
and the place that genuinely needs the prohibition is the parser, which already
carries it as a comment where a contributor would reintroduce it.

## The one dropped scenario

`Capability not configurable to cross-bundle scope fails uniformly` is deleted
rather than rewritten. Its `WHEN` clause names a capability whose policy-schema
cap is below `all`; no capability has a schema cap, so the precondition cannot
be established and the scenario cannot be exercised.

There is nothing to rewrite it into. The behavior it purports to describe —
uniform failure against the `all` threshold — is already covered by
`Cross-bundle operation denied under home scope`, which reaches the same
outcome through a precondition that can actually hold.

`scripts/verify-openspec-deltas.py` reports this as the single dropped scenario
for the change. That is the intended retirement, confirmed rather than
overlooked.

## Two sites no delta governs

The cap is also asserted in `relay-routing-layer`'s Purpose preamble and in the
module documentation at `src/relay/routing.rs`. Neither sits inside a
requirement, so neither is reachable by a delta; both are direct edits, and the
`routing.rs` comment is the only file outside `openspec/` this change touches.

The `routing.rs` case is worth naming explicitly: it is code documentation
asserting a mechanism the code in the same module does not implement, sitting
two files away from the parser comment that forbids it. It would have survived
any sweep scoped to specifications.

## Method

Both deltas were seeded verbatim from the live requirements and then edited, per
`agentmux:procedures/general/4`. The `Authorization Stage` requirement runs 85
lines and `Uniform Cross-Bundle Authorization Model` 95, against an intended
edit of one paragraph each, so authoring either from meaning would have risked
dropping scenarios that have nothing to do with this change.
