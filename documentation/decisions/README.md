# Decision Records

Closed questions and the reasoning that closed them, kept so we do not
relitigate them.

A record here answers "why is it this way, and what did we already reject?"
It is not a design document and not a specification.

## When to write one

Write a record when **all** of these hold:

- a question was settled after real deliberation, and
- the option we rejected is attractive enough that someone will propose it
  again, and
- the reasoning would otherwise sit inside a specification, where it
  accumulates and nobody dares delete it.

The third condition is the practical trigger. Most of these records are
extracted from OpenSpec specs that had grown a rationale section.

## When not to

- **To describe how something works now.** That belongs in the subsystem
  `README.md` next to the code, which stays current because anyone changing
  the code is looking at it.
- **To justify one specific rule.** Keep that inline, attached to the rule it
  defends. Someone editing the rule is then confronted by the reason; a
  justification one directory away is not. A justification that cannot be made
  in about three sentences is usually a rejected alternative in disguise —
  that is the signal to write a record instead.
- **To record what a document used to say.** That is git.

## The rule that keeps these honest

> A record states a decision and the alternatives it rejected. It never
> describes current architecture.

Decision records rot when they describe the present, because the present
changes and nothing forces anyone to notice. A record about a closed question
has a fixed referent: "we rejected byte-budgeted round-robin, because …" stays
true no matter what the system does afterwards.

If a draft record starts describing how a subsystem is built, it is a
subsystem README, not a decision.

In practice this means writing the decision's **shape**, not its current
spelling. "Startup does not block on worker initialization" survives a rename;
"startup returns `TransportReadiness::Pending`" does not, and a record written
the second way is false the next time that type is touched. Name a live symbol
only where it identifies the thing that was *rejected* — a rejected artifact
cannot drift, because it is already gone.

## Format

Deliberately informal. One file per decision:

    documentation/decisions/NNNN-kebab-case-title.md

Numbers are sequential and zero-padded to four digits. Numbers rather than
dates or slugs because supersession reads better numerically, and because a
number is the one identifier that cannot rot — unlike a change id, which gets
archived, or a path, which gets renamed.

A record needs a title, a date, a status, and prose. Suggested shape, to
deviate from freely:

```markdown
# 0007. Reject byte-budgeted round-robin scheduling

- Date: 2026-08-30
- Status: accepted
- Supersedes: —
- Superseded by: —
- Specs: delivery-quiescence / Async Queue Lifecycle and Ordering

## Decision

...one or two sentences...

## What we rejected, and why

...the part worth keeping. Be concrete about the failure mode...
```

The `Specs:` line names the requirement the decision stands behind, so a
reader who arrives from the spec can get back. Keep a matching one-line
pointer in that requirement — an unlinked record is a graveyard.

## Superseding

Do not edit an accepted record's decision. Write a new record, set the old
one's status to `superseded by NNNN`, and set the new one's `Supersedes`.
Correcting a typo or fixing a broken link in place is fine.

## Relationship to the other homes

| Content | Home |
|---|---|
| Required behavior | OpenSpec spec |
| Justification for one rule | Inline, in that requirement |
| Rejected alternative, closed question | Here |
| How a subsystem is built | `src/**/README.md` |
| How an algorithm works | Comment beside the code |
| What a document used to say | git |
