## Context

Peering is registered mutually: each relay issues the other an identity and a
credential, so one relationship is described by two exchanges. Each relay ends up
holding two records about its counterpart — the principal it issued (inbound
authentication) and the `[[peers]]` row it dials through (outbound routing) —
and nothing states that they concern the same peer.

That absence is invisible until something needs to name an inbound peer. Then it
is total: the authenticated principal is a name this relay chose, the row's
`connect-as` is a name the peer chose, and no local data relates them.

It stays invisible longer than it should because a symmetric naming convention
makes the two coincide. A deployment that names both sides alike works, and
reports nothing about the deployment that does not.

## Goals / Non-Goals

**Goals.** Make a peer's name derivable from data the relay already holds, check
the constraint rather than trusting a convention, and keep the two directions of
the relationship distinguishable.

**Non-Goals.** Envelope rendering, which this unblocks and which follows
separately. Replacing mutual registration with one-sided registration, which
would remove the outbound table on the issuing side and supersede this
arrangement rather than build on it.

## Decisions

### The issued identity is the name

The relay already holds exactly one authoritative fact about a peer's identity:
the principal it issued. Making that the name removes the join instead of
bridging it — there is nothing to look up, because the authenticated principal
*is* the answer.

It also makes the name structurally unique. The principal store is keyed by
principal id, so a relay cannot issue two peers the same identity. The previous
arrangement relied on operators keeping aliases distinct, and on `connect-as`
values not colliding — the latter being something no relay controls, since each
peer decides independently what identity to issue this one.

**Alternative considered:** adding a back-reference — recording the alias on the
principal record, or the issued identity on the `[[peers]]` row. Rejected: it
keeps two records and adds a third fact to hold in agreement, so it moves the
failure from "cannot derive" to "can derive, and it drifts." The constraint form
has one fact and a check.

### Validate the invariant, and validate only the invariant

The check is that `<alias>@RELAY` names an existing relay principal. It stops
there deliberately.

In particular the outbound credential is not compared against the store record's
credential hash. Those are opposite directions: the credential on disk is what
the peer issued this relay, and the store record is what this relay issued the
peer. Requiring them to agree would assert the very relationship the data model
does not record, and would fail every correctly configured deployment.

The temptation to check more is worth naming, because the check that looks
natural is the one that is wrong.

### The check is unconditional, which costs a registration

The invariant only *matters* for a peer that can send to this relay, and those
are exactly the peers this relay has issued an identity to. A peer this relay
only dials — it sends, outcomes return on the connection it opened, the peer
never initiates — is never named, so its alias is never used.

That suggests a conditional check: require the alias to name an issued principal
only where such a principal exists. It was rejected, because it cannot fail.

The misconfiguration worth catching is a relay that has issued a peer `bravo`
while dialing it through a row aliased `peer-b`. Inbound, that peer is named
`bravo`; a reply addressed to `bravo` finds no row and fails as an unknown peer.
To catch it, validation would have to know that row `peer-b` and principal
`bravo` describe the same peer — which is the absence this whole change exists
to work around. Under a conditional rule, `peer-b` is simply read as dial-only
and accepted, so every alias passes: it either names a principal, or it does not
and is excused. A check that cannot reject is not an invariant.

The unconditional rule can reject it, at the price of requiring a registration
for a peer that would not otherwise need one. That price is one command per peer
at setup, and it buys the only version of the rule with teeth.

**A missing store record is not evidence of a dial-only peer.** It is equally
consistent with a mistyped alias, a stale one left behind after a peer was
re-registered, or a row copied between deployments. Nothing local distinguishes
them, so treating absence as permission accepts all three.

This is the arrangement that one-sided registration removes: there, a relay
issues to peers it receives from and holds no outbound table at all, so the
question does not arise.

### Configuration validation reads relay state

This gives configuration validation a dependency it did not have: it currently
reads files, and will now consult the principal store.

Accepted rather than worked around. The alternative is deferring the check to
first delivery, which is exactly when the operator is least able to act on it —
and a peer that is misconfigured this way is misconfigured from startup. The
check belongs where the deployment is judged, alongside the existing peer
address and field validations, and it surfaces through `check configuration`
with them.

### Breaking, with no fallback

The credential path is stemmed by the alias, so this moves it on disk for any
deployment where the alias differs from the issued identity.

No automatic migration is possible, for the same reason the change exists: the
old alias and the issued identity have no join, so nothing local can compute the
new value. A migration would have to guess.

A dual-stem fallback — try the new path, then the old — was considered and
rejected. It would let a relay silently find a credential under a name the
configuration no longer claims, which is the same class of silent coincidence
this change removes, reintroduced as a compatibility measure. The failure is
instead explicit and names the path that is missing.

### Compose the sender where the delivered message is built

`AddressIdentity` holds one string read two ways: a non-decorating accessor feeds
machine consumers (the `incoming_message` event, the envelope metadata record)
while the decorating renderer produces the pane header. Two accessors over one
value, not two values.

The identity is therefore composed at the construction site rather than inside
pane rendering. Composing at render time would correct the pane header and leave
the stream event and the audit record still attributing the message to the
forwarding relay — three surfaces disagreeing about who sent one message, with
the audit trail the least likely to be noticed and the most consequential when
wrong.

### Both segments, not the origin alone

The receiving relay authenticates the peer, never the foreign origin, so
`on_behalf_of` is an advisory claim a peer could set to anything. Rendering the
origin alone would present it with the authority of a locally verified sender.

Today's behavior, malformed as it is, does not have that defect: it names the
principal this relay actually authenticated. The composed form keeps that
property while fixing the attribution, because it names the origin *and* the
peer that vouched for it. A reader sees the provenance; a reply parses back
through the grammar the router already implements.

This is also what reconciles the change with the standing prohibition on
interpreting `on_behalf_of`. Composing a display string is not resolution: no
segment is validated against the local principal store, and nothing about the
composed identity reaches an authorization decision.

### Reply-derivability is real here, and was not before

An earlier attempt at this rendering derived the peer segment from the
authenticated principal itself. That is invalid — the principal a peer presents
and this relay's name for that peer are separate naming systems — so the
resulting address would not have resolved, and a target-shaped string that does
not resolve is worse than none: it invites replies that fail, or that resolve to
a different peer.

Binding the alias to the issued identity is what makes the derivation sound, and
is why that half is a prerequisite rather than an accompaniment. The property is
verified by feeding the rendered identity back through cross-relay target
resolution rather than by asserting the format, so the two grammars cannot drift
apart silently.

Where the named peer has no outbound entry, the address is still correct and
still does not resolve — and fails as an unknown peer, which is the honest
outcome for a relay with no route back.

### Resolution accepts any origin namespace

Cross-relay forwarding attributes any verified requester, including a relay-wide
`@GLOBAL` user, while target resolution accepts only bundle-qualified sessions.
That gap makes a correctly rendered sender unparseable as a target for exactly
the senders least likely to be noticed in testing.

Widening resolution is therefore part of this change rather than a follow-on: the
envelope form promises a reply address, and a promise that holds for some origins
and not others is not the property being claimed.

## Risks / Trade-offs

**Existing deployments break on restart.** Deliberate, and consistent with the
project's stated position on backwards compatibility before 1.0. The mitigation
is a failure that says which alias is wrong and what is missing, not a silent
degradation.

**A convention that currently holds will keep the invariant satisfied by
accident.** A symmetrically named deployment satisfies the new check without the
operator learning anything. That is acceptable — the check exists for the
deployments where it does not hold — but it means testing must construct the
asymmetric case deliberately, since the natural fixture passes either way.

**The check is only as good as its teeth.** A validation that accepts everything
looks identical to one that is satisfied. Each new assertion is verified by
making it fail on purpose, individually rather than through a loop, since a loop
stops at its first failure and says nothing about the cases after it.
