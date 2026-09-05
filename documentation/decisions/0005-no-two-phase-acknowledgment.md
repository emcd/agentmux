# 0005. Acknowledgment is one phase

- Date: 2026-09-05
- Status: accepted
- Supersedes: —
- Superseded by: —
- Specs: delivery-quiescence / Delivery Guard and Acknowledgment Terminalization

## Decision

An acknowledgment establishes a packing unit's result and resolves every member
it covers as one indivisible act. There is no state in which a unit's outcome is
known and a member it covers is still unresolved.

The consequence worth naming, because it is what someone will try to add back:
the guard's resolution order has exactly two rungs, and both discriminate on
whether a member was declared into a unit at all. It has no rung for "the unit
already produced a result", because no member can reach the guard in that
condition.

## What we rejected, and why

A two-phase acknowledgment: a transport reports the unit-level result in one
call and per-member outcomes in a later one.

It is a reasonable thing to want. A transport often knows that a write went out
before it knows what became of each member — a framed request succeeds as a
whole, and attributing per member may need a response that has not arrived. Two
phases let the relay bank the strong fact immediately rather than holding it.

The cost is a window in which a unit has a recorded outcome and unresolved
members, and every lifecycle trigger — shutdown, bundle stop, the execution
bound, generation replacement — can land inside it. Making that window safe
requires the guard to consult the unit's recorded result and prefer it over the
trigger: a third rung above the two. So the rung and the second phase are the
same decision asked twice, and neither is worth having without the other.

It also makes disagreement representable. Phase one says the unit submitted;
phase two says its third member did not; the relay has now recorded both, and
nothing in the model says which answers for that member. The single-phase form
cannot express the contradiction because there is only ever one report per
member.

What one phase costs is a wait: the transport holds its report until it can
attribute per member. That wait is bounded by the transport's own knowledge
rather than by any relay timer, and a transport that cannot attribute per member
has a cheaper move available — declare a smaller unit, down to a single member,
which expresses the same partial outcome without a second phase.

## The rung that used to be there

The third rung was not a mistake when it was written. Under the push model the
relay packed the unit and fanned out to its members, recording the unit's
evidence before any member resolved; a trigger really could catch a sibling
mid-fan-out, and without the rung a delivery known to have succeeded would have
been downgraded to `submission_unknown` by whichever lifecycle event fired. The
fan-out window and a two-phase acknowledgment's window are the same shape.

Moving to a pull model closed it, by making the recording and the resolution one
locked act. The rung was carried forward across that cutover and became a field
that was written and never read — which is how it was found, and why this record
exists rather than a comment: the next person to notice the asymmetry between
"evidence is per unit" and "outcomes are per member" will reach for exactly the
two-phase design again.

## What this does not decide

Not that a packing unit holds one member. It may hold many, and today's ledger
minting one member per unit is current architecture rather than a decision —
`src/relay/README.md` and the ledger's own comments carry that. Multi-member
units do not on their own reintroduce the window: what reopens it is splitting
the *report*, not widening the unit.

Not how a transport decides where to cut a unit. Packing policy belongs to the
transport, and this record constrains only when it may speak about the result.
