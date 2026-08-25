## Context

`bind-peer-alias-to-issued-identity` (archived 2026-08-24) fixed cross-relay
sender *rendering* — a delivered message now names both the origin and the
peer that vouched for it, composed as `<on_behalf_of>!<peer-alias>`, and that
composed identity is a resolvable reply address whenever the origin is a
routable canonical principal id. It deliberately left the *stamping*
condition alone: `on_behalf_of` is populated only from a verified
(`store_backed`) principal, so a socket-trust sender's message is delivered
with `on_behalf_of` absent and falls back to naming the bare peer-relay
principal.

That fallback was inherited, unchanged, from before the rendering fix — and
it was already the wrong shape then. It is `<id>@RELAY`: syntactically a
valid target, and one that actually resolves (to the peer relay's own
principal), so nothing about it signals "do not reply here." Under this
project's actual configuration — `require-session-credentials = false` on
every relay we run — it is also not a rare fallback; it is what every
ordinary agent-to-agent cross-relay message produces today.

## Goals / Non-Goals

**Goals.** Make ordinary agent-to-agent cross-relay messaging repliable in
this project's actual (single-operator, Unix-socket-only) deployment, without
weakening the wire-level guarantee that `on_behalf_of` is never treated as an
authorization input. Make the non-repliable case, wherever it still occurs,
honest about itself rather than inviting a failed reply.

**Non-Goals.** Multi-tenant or network-exposed relay deployments — out of
scope for now, consistent with this project's alpha posture. Changing
anything about how the *receiving* relay treats `on_behalf_of` (still
advisory, still uninterpreted, still never an ingress input) — only the
*forwarding* relay's stamping condition is in question.

## The threat-model argument, as put to the operator

Withholding `on_behalf_of` from a socket-trust origin protects against a
principal impersonating another session to a peer relay's operator. On a
Unix-socket-only, single-operator deployment, whoever can reach the relay's
local socket is ordinarily the same OS user as the relay process, with direct
read access to the credential store and PSK files under the state root —
i.e., already capable of everything the current gate prevents, by more
direct means (reading a real credential, or editing `principals.json`
outright). The gate's cost is real (the UX failure this proposal exists to
fix); its benefit, in the deployment this project actually runs, is close to
zero.

The gate keeps value in a deployment where the socket and the credential
store sit behind *different* local trust boundaries — a sandboxed or
containerized session that can reach the relay socket via a bind-mount but
not the state directory holding the PSK files, for instance. Nothing in the
current architecture rules that out as a future deployment shape, which is
why the proposed change is an explicit per-peer opt-in rather than lowering
the default.

**This argument has not been reviewed by BE or AuxBE.** It is presented here
as the operator's and the Coordinator's reasoning to react to, not as a
settled conclusion — see Open Question 5 in `proposal.md`.

## Alternatives considered (informally, not yet exhaustive)

**Leave stamping alone; fix only the fallback rendering.** Solves the "invites
a failed reply" problem without touching the trust boundary at all. Cheaper
and lower-risk. Does not solve the actual goal (ordinary sessions still
cannot be replied to across relays) — would need a separate, later change to
get there, and the operator's stated preference is to solve repliability
directly. Worth weighing against the opt-in approach on effort/risk grounds
during review.

**Relay-wide toggle instead of per-peer.** Simpler surface, matches
`require-session-credentials`'s own scoping. Loses the ability to trust one
peer's socket boundary but not another's — probably fine for a
single-operator deployment talking to peers it also operates, less fine if a
peer relay is ever operated by someone else. Raised as Open Question 1.

**Do nothing; document the limitation.** Rejected by the operator in
conversation — the current behavior is confusing enough in practice (an
agent guessing a reply target and reaching the wrong recipient) that leaving
it as documented-but-broken was not acceptable.

## Risks / Trade-offs

Whatever the resolution, it touches identity/attribution code one day after
a related capability shipped — worth being deliberate about test coverage
for the opt-in boundary specifically (a peer with the flag set vs. one
without, in the same relay, must not cross-contaminate).

An opt-in that defaults to `false` is safe to ship incrementally: no existing
deployment's behavior changes until an operator sets it, so this does not
carry `bind-peer-alias-to-issued-identity`'s breaking-on-restart cost.
