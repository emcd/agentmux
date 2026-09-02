## Context

This is a trial. If a verb spec works for `raww` it will be repeated for `send`,
so the interesting question is not whether these six requirements can be moved —
they can — but where the boundary falls and whether it survives contact with a
verb whose relay-side home is genuinely contested.

## The boundary

A verb spec owns the **relay-side semantic contract**: what the verb does, its
target rules, its outcomes, its authorization control. Each surface spec keeps
its own adapter shape.

The test that decides any individual statement: would this still be true if the
verb were reachable only through a different surface? The `raww` input bounds
hold whether the request arrives by CLI, MCP or TUI, so they belong to the verb.
The MCP request payload's field names hold only for MCP, so they belong to the
surface. That is the same split the corpus already asserts wherever it says the
surfaces validate and the relay decides; a verb spec makes the relay-side half
addressable.

## Three kinds of raww mention, and only one of them moves

Sweeping for the verb rather than for raww-titled requirements turned up a
seventh capability, `relay-routing-layer`, with nineteen mentions and no
raww-titled requirement. Those mentions are not one thing:

**General rules that enumerate raww.** "The relay SHALL resolve all
target-addressed operations (Send, Look, Raww, List)", the handler list, the
cross-relay classification. Moving these would break the general rule for the
other three verbs. They stay.

**Raww-specific semantics inside a general requirement.** The note explaining
that `can_be_written = false` targets are rejected by the raww capability gate,
so `home` confers no effective reach and `all` is the meaningful tier for a
relay-wide principal. That is raww's contract sitting in the routing layer's
requirement. It moves.

**The general rule re-instantiated for raww.** `Cross-bundle Raww denied under
home` and `Cross-bundle Raww permitted under all`. These required a judgment
rather than a rule, and the two resolved differently — see below.

The distinction that decided them: does the requirement illustrate its rule once
per verb? `Authorization Stage` demonstrates home-namespace evaluation with a
Raww scenario, a relay-wide send scenario and a List scenario. Removing only the
Raww one would leave that set lopsided, so `Requester authorized in home
namespace for cross-bundle Raww` stays where it is. The denied/permitted pair
has no send or list sibling in that requirement — it is raww-specific content
that landed there because the author was working there.

## One move turned out to be a deletion

`Cross-bundle Raww permitted under all` asserts that a requester with `raww` at
`all` targeting another bundle is routed and delivered. The `raww` authorization
mapping already contained `Cross-bundle raww permitted under all`, asserting the
same thing with the same preconditions. Same rule, two capabilities, written
twice.

So it is retired rather than relocated, and the destination's existing wording
is kept. Relocating it would have produced two identical scenarios inside one
capability, which is a worse outcome than the duplication we started with.

This is the argument for the whole change, arrived at accidentally. Nothing
detected that duplication for as long as it existed, and nothing would have:
`openspec validate --strict` checks that requirements are well formed, never
that two of them say the same thing. Consolidating a verb into one capability is
what makes such pairs visible, because it puts them on the same page.

## Why the generic scenario is de-enumerated rather than extended

`Cross-bundle operation denied under home scope` named `look`, `send` and `list`
and omitted `raww`. The obvious repair is to add `raww` to the list. That is the
wrong direction: the list is the defect, and every future verb would have to
remember to join it — which is precisely what did not happen for `raww`, and
plausibly why raww's own copies appeared in `relay-routing-layer`.

Replacing the enumeration with "any cross-bundle target operation" makes the
rule cover verbs that do not exist yet, and removes the maintenance obligation
that was already being missed.

Two further enumerations in the same requirement are deliberately untouched:
`Cross-namespace session raww/look authorizes in the home namespace` carries an
extra precondition and asserts something narrower, and `Relay-wide principal
needs all-all to reach a bundle` uses its verb list illustratively. Both want a
judgment that belongs with the send de-duplication rather than here.

## Method

Every relocated requirement was extracted verbatim per
`agentmux:procedures/general/4` and then verified in the other direction: each
requirement in the new capability was diffed against its live source. Five of
six are byte-identical; the sixth differs only by the deliberately relocated
scenario and note.

That check is worth keeping for any future verb extraction. A move is a REMOVED
delta in one capability and an ADDED delta in another, and no tooling confirms
the two halves carry the same text — `scripts/verify-openspec-deltas.py` audits
retention within a capability, not across a move.
