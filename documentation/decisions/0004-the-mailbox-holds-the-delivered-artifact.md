# 0004. The mailbox holds the delivered artifact, not a copy of it

- Date: 2026-08-31
- Status: accepted
- Supersedes: —
- Superseded by: —
- Specs: delivery-quiescence / Mailbox Ordering and Cursor Lifecycle

## Decision

An entry has one payload. It is built once and stamped once, it is in place
before the entry becomes peekable, and whatever writes the entry writes that
payload. No writer builds an envelope of its own.

## What we rejected, and why

A shadow enqueue: store a payload nothing delivers, and let the writer keep
building its own at the write.

It is cheaper, it is obviously safe — it changes no delivered byte — and it is
the natural thing to reach for when a mailbox lands ahead of the consumer that
will drain it. It also proves nothing. Two envelopes built from one task at two
moments agree on everything a test would think to compare and differ in whatever
it does not. The whole reason to move the artifact ahead of the consumer is to
put the consumer's future input under the existing suite; a payload nothing
delivers is not that input, it is a parallel object that happens to resemble it.

The failure the shadow would hide is the one worth catching: that the record the
relay keeps of a delivery does not describe what went on the wire. A mailbox
whose contents are never delivered cannot surface it, and a mailbox whose
contents are always delivered cannot help surfacing it.

## The cost that makes this a decision rather than an obvious call

Building before the write means stamping before the write, and the timestamp is
observable. A message's `Date` now names when the relay built its envelope, not
when a transport got round to writing it; for an entry that waits out a busy
target the two differ by however long the target stayed busy.

That was the argument for building late, and it loses. Build time is the only
reading that can be the same on both sides of the mailbox, which is the whole
property being bought. It is also the more useful of the two: write time reports
the target's availability, which the terminal outcome already carries, while
build time reports when the relay accepted the message.

The second cost is that any out-of-band record emitted where the payload is
built now describes envelopes that are subsequently not delivered: an envelope
exists for every entry the relay accepted, and not every such entry reaches its
target. That is the correct reading of such a record — the envelope exists and
the relay is holding it — but it is a weaker reading than the one it had when
only a written envelope produced a record. A reader correlating it against
deliveries pairs it with the terminal outcome, which was already required for
any entry whose write failed.

## What this does not decide

Not where the payload is built — that is current architecture, and it lives in
`src/relay/README.md` with the rest of the delivery worker's shape. Only that
there is one of it, and that the mailbox holds it rather than resembling it.

Not where an entry sits in its target's order, nor what settles that. Ordering
and the cursor are governed by `Mailbox Ordering and Cursor Lifecycle`, and this
record bears on neither: custody and position are independent questions about
one entry. Reading it as though building a payload were what ordered an entry
would import a claim it does not make.
