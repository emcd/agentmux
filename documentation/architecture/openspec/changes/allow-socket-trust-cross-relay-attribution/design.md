## Context

`bind-peer-alias-to-issued-identity` (archived 2026-08-24) settled cross-relay
sender *rendering*: a delivered message names both the origin and the peer that
vouched for it, composed as `<on_behalf_of>!<peer-alias>`, and that composed
identity is a reply address whenever the origin segment is a routable canonical
principal id. It left the *stamping* condition alone.

This change is about that condition, and about one fact the earlier change could
not have addressed: the guarantee it established is conditional on a conforming
forwarding relay supplying an origin, and our own relays never supply one.

### What the current behavior actually is

Three claims are worth stating precisely, because the shape of the fix depends on
which of them are true.

**An unauthenticated origin yields no `on_behalf_of`.** True, and by a single
chain: `verify_socket_trust` returns `store_backed = false`; the Hello path
derives the connection's identity as `store_backed.then(...)`, so
`authenticated_identity` is `None`; the cross-relay forwarding site passes that
`None` as the origin. A separate guard discards a self-asserted `on_behalf_of`
from any non-ingress requester — `send.rs` substitutes `None` and `raww.rs`
drops the field at destructuring, neither refusing the request — so a session
cannot route around it by supplying its own.

**Socket-trust is the ordinary path, not an edge case.**
`require-session-credentials` defaults to `false`, the deployed `relay.toml`
leaves it commented out, and no running relay passes `--require-credentials`.

**The unattributed sender resolves as a target.** *Not true.* Both the plain form
and the bang-path form are refused at target resolution with
`validation_unsupported_namespace` — the peer-relay namespace names no routable
recipient, and the code refuses it identically on the local and cross-relay
paths. The refusal is accurate and specific, and the live incident corroborates
it: the recipient's two reply attempts were rejected before it resorted to
guessing.

That correction removes any misrouting defect from the receiving relay — a reply
is never delivered to the wrong principal by this mechanism — and it substantially
weakens the option of fixing only the rendering, because the error a reader would
get is already the right error. What remains is a real but narrower problem:
attribution that *looks* addressable and is not.

### Where the fallback came from

It was not inherited residue. `cross-relay-routing` states it as an obligation —
when `on_behalf_of` is absent, attribute to the peer relay principal, qualified
exactly once — in the same requirement that defines the composed form. It is a
deliberate choice to preserve provenance over addressability, and it is right for
the case it was written for. It is wrong only as a *default* experience, which is
a fact about our credential posture rather than about the rule.

## Goals / Non-Goals

**Goals.** Make ordinary agent-to-agent cross-relay messaging repliable in this
project's actual deployment, without weakening the guarantee that `on_behalf_of`
is never an authorization input and without letting an unverified session acquire
a verified-looking identity.

**Non-Goals.** Multi-tenant or network-exposed relays. Any change to the
receiving relay. Per-origin ingress filtering that consumes `on_behalf_of`, which
`cross-relay-routing` already places out of scope.

## Why attribution follows admission

The rule being replaced withholds `on_behalf_of` from an origin the relay did not
verify. Stated that way it sounds like a security boundary. It is not one, and
identifying what it actually is decides the whole shape of this change.

A relay running with `require-session-credentials = false` accepts a session's
claim about its own identity **and acts on it everywhere**. `verify_socket_trust`
performs no credential check; the session is registered under the id it claimed;
local delivery attributes from that claimed id; the stream registry keys on it;
every local recipient sees it. The claim is not provisional pending some later
check — it is the identity that session has on that relay.

Against that, withholding the same claim from a peer withholds no capability. The
session can already act as its claimed identity toward every local participant.
What the old rule achieved was not containment but inconsistency: the relay
delivered under an identity locally while telling peers it did not know who sent
the message.

So the decision belongs where it is already made. `require-session-credentials`
is the operator's statement about whether claims on this socket are good enough
to act on. Attribution should follow that answer rather than re-litigate it, and
a second setting could only express "admit this identity but do not stand behind
it" — a distinction the relay draws nowhere else, and one whose two sides are
indistinguishable to every local recipient.

### The residual asymmetry, which is real and pre-existing

A peer cannot tell whether our relay verified the origin or accepted its claim.
`on_behalf_of` from a credential-requiring relay and from a permissive one look
identical.

That asymmetry is genuine, and the specification already resolves it in the only
way it can: `on_behalf_of` is advisory, is never an authorization input, and the
receiver is forbidden to interpret it. The receiver cannot verify a foreign
origin in *either* case — a store-backed attribution from a peer is exactly as
unverifiable to it as a socket-trust one. A wire marker distinguishing them would
therefore convey nothing the receiver may act on while inviting it to treat one
as weaker evidence, so the delta forbids one.

What actually bounds the asymmetry is the peer relationship itself: peers are
registered deliberately, hold issued credentials, and are named by the receiving
relay rather than self-named. A peer that attributes carelessly is a peer to
stop registering, which is a decision at a coarser and more visible grain than
any per-message signal.

## What this change gives up

Deciding attribution at admission means there is no per-peer control over it. An
operator who wanted a peer to receive messages *without* seeing internal session
ids cannot express that. This is a disclosure question rather than a trust one,
and it is worth naming as the price of the simpler design.

It is accepted here because every peer this project federates with is operated by
the same person who operates this relay, and because the control can be added
later without a breaking change: a per-peer suppression field would layer over
the admission-derived default rather than replacing it. Building it now would be
building a knob with no current caller and no current disagreement to resolve.

## Alternatives considered

**A separate opt-in key gating attribution independently of admission.** This was
the shape the proposal carried through its first two review rounds, and it was
rejected once its justification was examined. The argument for it was that
socket-trust attribution needs a distinct operator decision because socket access
implies credential-store access — so that where the two diverge, the relay should
not vouch. The premise does not survive: credential-store reachability is not why
self-assertion is acceptable. `require-session-credentials = false` is. The
equivalence was a coincidental second reason mistaken for the load-bearing one.

The concrete case that exposed it was containerized sessions, where the relay
socket is bind-mounted in and the state root is not. That was taken to break the
justification and to force the opt-in and sandboxing to be mutually exclusive.
It does neither. A sandboxed agent is a strict *reduction* in privilege — the
same process previously ran with the relay's own filesystem access — and the
capability at issue, asserting an identity, it already held and still holds
locally. Bind-mounting a socket into a container grants nothing; it removes
things. There is no divergence to design against, and so nothing for a
configuration check to detect and nothing for the two changes to conflict over.

**Fix only the fallback rendering, leave stamping alone.** Weaker than it first
appears. It was attractive on the belief that the unattributed sender resolves
and invites a misrouted reply; it does not resolve, and the reply already fails
with an accurate, specific error. What such a change could still add is making
the sender visibly non-addressable so no reader forms the belief in the first
place — worth doing eventually, but it does not make ordinary messages repliable,
which is the actual goal.

**Provision real session credentials for agent sessions instead.** This would fix
repliability by making every session verified, and it remains the better end
state for reasons beyond attribution. It is out of scope here because it is a
bring-up and credential-lifecycle problem across every session-spawning surface
rather than a relay behavior change. Note that it does not compete with this
change: under credentials, admission is store-backed and attribution follows it
unchanged. This change makes the relay consistent with whatever admission policy
is in force, which is the behavior a credential rollout would want anyway.

**Do nothing; document the limitation.** Rejected by the operator, and the
incident supports the rejection: an agent handed an unusable sender and a correct
refusal guessed a target and reached an uninvolved session.

## Risks

The existing relay test harness connects as socket-trust throughout. That is
exactly the condition this change gives meaning to, so tests written against the
harness's default will exercise the newly-attributed path and prove nothing about
the credentialed one. Distinguishing them requires provisioning a real credential
in the test, as `preserve-self-rotated-credential` had to.

The two attribution fields become easy to conflate now that both are populated
for a socket-trust sender in one direction and not the other. `authenticated_
identity` remaining absent while `on_behalf_of` is present is the invariant most
likely to be broken by a later well-meaning simplification, and it is load-bearing
twice over: `relay-identity` requires it, and live-stream revocation matches on
`authenticated_identity`, which is why socket-trust connections are never swept.
Asserting both in one delivered envelope is what pins them as separately sourced.
