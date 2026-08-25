## Why

Discovered live during a cross-relay smoke test: a `Send` from an ordinary
local session (`coordinator@agentmux`, socket-trust — the default for every
bundle member on both R&D relays, since `require-session-credentials` is
`false`) to a peer relay carries no `on_behalf_of`. The receiving relay falls
back to attributing the message to the bare peer-relay principal
(`rnd-main@RELAY`). That fallback string parses as a valid `id@namespace`
target and even resolves to a real principal, so a recipient reasonably tries
to reply to it — and fails, having no way to know the reply is unroutable
before sending it. In the actual incident this produced, the recipient
improvised a guessed fallback target that reached an unrelated third session.

This is not a code defect against `cross-relay-routing`'s Sender Attribution
Forwarding requirement (`openspec/specs/cross-relay-routing/spec.md:94`),
landed by `bind-peer-alias-to-issued-identity` (archived 2026-08-24) — the
requirement explicitly withholds `on_behalf_of` for an unverified origin, and
socket-trust is unverified by definition (`src/relay/identity.rs`
`verify_socket_trust`: the claimed `principal_id` is accepted with no
credential check at all). But it means that, under the configuration this
project actually runs — Unix-socket-only, `require-session-credentials =
false` on every deployed relay — **no ordinary local session's cross-relay
message is currently repliable**. That is not an edge case; it is the default
and, today, the only path any agent session takes.

The security rationale behind gating on verification is sound in general —
socket-trust performs no verification of the asserted identity — but its
value depends on the trust boundary the socket actually sits behind. On this
project's actual deployment (single operator, Unix-domain sockets only, no
network exposure), anyone who can reach the relay's local socket is
ordinarily already running as the same OS user as the relay itself, with
access to the credential store and PSK files directly. Withholding
`on_behalf_of` from that principal buys little: the capability it protects
against (impersonating a session to a peer) is already available to anyone
positioned to exploit it, by more direct means. A deployment where the socket
and the credential store sit behind different local trust boundaries (a
sandboxed or containerized session bind-mounted onto the relay socket but not
the state directory, say) is the case where the current default earns its
keep — so the fix should be a deliberate opt-in per outbound peer, not a
blanket relaxation.

## What Changes

Draft direction — not yet decided; see Open Questions and `design.md`.

- A new `[[peers]]` field (proposed name: `allow-nonauthenticated-access`,
  default `false`) lets an operator declare, per outbound peer, that this
  relay's socket-trust boundary is trusted enough to vouch for a session's
  self-asserted `principal_id` when forwarding to that specific peer. When
  set, the forwarding relay stamps `on_behalf_of` from the session's claimed
  `principal_id` even when `store_backed` is `false`, instead of forcing it to
  `None`.
- Independent of the above: when `on_behalf_of` is genuinely absent (an
  unauthenticated origin, or a peer that does not opt in), the delivered
  sender identity stops falling back to a bare, target-shaped principal id.
  It should be rendered in a way that visibly cannot be replied to, so
  neither a human nor an agent is invited to construct a reply from it — and
  an attempted reply to it should fail with a message naming the actual
  problem ("sender could not be identified for reply"), not a generic
  unknown-target validation error.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `runtime-bootstrap`: *Outbound Peer Relay Configuration* gains the new
  `allow-nonauthenticated-access` field and its validation.
- `cross-relay-routing`: *Cross-Relay Sender Attribution Forwarding* gains the
  opt-in condition under which a socket-trust origin is stamped as
  `on_behalf_of`, and a requirement covering the non-repliable fallback's
  rendering and reply-failure behavior.
- `relay-identity`: *Sender Attribution Schema* documents the widened source
  of `on_behalf_of` and reconciles it with the existing non-resolution /
  non-authorization prohibitions (this does not change — `on_behalf_of`
  remains advisory and untrusted by the receiver regardless of how the
  forwarding relay populated it).
- `pane-envelope` / `look-and-stream-events`: whatever rendering change
  replaces the bare-peer-principal fallback.

## Open Questions (for BE / AuxBE)

1. Is per-peer opt-in the right scope, or should this be relay-wide (a single
   `require-session-credentials`-adjacent toggle rather than a `[[peers]]`
   field)? Per-peer keeps the default conservative for peers whose trust
   boundary is unknown; relay-wide is simpler and matches how
   `require-session-credentials` itself is scoped.
2. Field name and shape — `allow-nonauthenticated-access` was the operator's
   working suggestion, not a commitment.
3. What should the non-repliable fallback identity actually look like? It
   needs to (a) not parse as a valid reply target, (b) still carry whatever
   provenance is available (which relay, at minimum), and (c) render sanely
   in the pane header, the `incoming_message` event, and the envelope
   metadata record — the same three-surface consistency requirement
   `bind-peer-alias-to-issued-identity` established for the repliable case.
4. Does opting a peer in change anything about `IdentityIntrospect` or the
   trusted-host `identity.snapshot`/`identity.revoked` surfaces, or is this
   scoped purely to `on_behalf_of` on Send/Look/Raww forwarding?
5. Threat-model sign-off: does the "local socket access implies credential-store
   access in this project's actual deployment" argument hold, or is there a
   deployment shape (sandboxing, container bind-mounts) worth designing
   against now rather than deferring?

## Impact

Not yet assessed pending the open questions above — deferred to `design.md`
and the eventual spec deltas once the shape is settled.
