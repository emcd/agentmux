## Why

Under the configuration this project actually runs, no ordinary local session's
cross-relay message can be replied to.

A `Send` from a socket-trust session to a peer relay carries no `on_behalf_of`.
The forwarding relay stamps that field from the origin's *verified* principal id
(`src/relay/handlers/send.rs`), and a socket-trust connection has none:
`verify_socket_trust` accepts the claimed `principal_id` without any credential
check and records `store_backed = false`, from which the connection's
`authenticated_identity` is derived as `None`. The receiving relay, seeing no
`on_behalf_of`, attributes the message to the bare peer-relay principal
(`rnd-main@RELAY`) — which is exactly what the `cross-relay-routing` capability
requires of it.

None of that is a defect. Each step is specified. The problem is that
`require-session-credentials` defaults to `false`, no deployed relay sets it, and
no agent session is provisioned with a credential — so the unattributed path is
not a fallback for an unusual case. It is the only path any agent session takes,
and the guarantee that a delivered sender is a reply address is therefore vacuous
in practice.

The observable failure is a recipient who cannot reply and cannot tell why. The
delivered sender is shaped like an address (`id@namespace`), so a reader forms
the reasonable belief that replying will work. It does not: both `rnd-main@RELAY`
and `rnd-main@RELAY!rnd-main` are refused at target resolution with
`validation_unsupported_namespace`, "target namespace names no routable recipient
for this operation". That refusal is correct, and it is specific rather than
generic. In the live incident behind this proposal the recipient absorbed two
such refusals and then *guessed* a target, reaching an uninvolved third session.
The relay misrouted nothing; the recipient, left with an unusable sender and an
accurate error, improvised.

The rule producing this withholds attribution from an origin the relay did not
verify. That is the right instinct in general and the wrong place to apply it,
because it re-decides a question the relay has already answered. A relay running
with `require-session-credentials = false` has admitted that session under the
identity it claimed, and delivers under that identity: locally, a socket-trust
session already *is* what it says it is — it sends, receives, and appears to
every recipient as its claimed `principal_id`. Withholding the same claim from a
peer does not withhold a capability. It only makes the relay's cross-relay
attribution disagree with its own local delivery.

## What Changes

- The forwarding relay stamps `on_behalf_of` with the `principal_id` the origin
  was **admitted** under, whether that identity was verified against the
  principal store or accepted as a socket-trust claim. The special case for
  unverified origins is removed rather than made configurable.
- `require-session-credentials` remains the single place the admission decision
  is made. A relay that requires credentials admits no unverified principal and
  so forwards only store-backed attributions; a relay that accepts socket-trust
  attributes what it accepted. No new configuration key is introduced, and none
  should be: a second setting could only express "admit this identity but do not
  stand behind it", which is a distinction the relay does not act on anywhere
  else.

Deliberately unchanged, each for a stated reason:

- **`authenticated_identity` stays absent for an unverified requester.** It is
  the field that records whether a credential backed the identity, it is what
  live-stream revocation matches on, and `relay-identity` requires a session
  without a verified principal to omit it rather than self-assert. The two fields
  stay separately sourced: one carries who the origin was admitted as, the other
  whether a credential backed it.
- **The absent-`on_behalf_of` fallback keeps its rendering and its refusal.** A
  peer may still omit the field — an older implementation, or one that declines
  to attribute — and attributing such a message to the peer relay, qualified
  once, remains correct. A reply to it already fails with a specific error naming
  the real problem.
- **The receiving relay's treatment of `on_behalf_of` is untouched.** It stays
  advisory, uninterpreted, never an authorization input, and never resolved
  against the local store. What a forwarding relay is willing to assert is a
  separate question from what a receiver may conclude, and only the former moves.
- **A requester still cannot supply its own `on_behalf_of`.** A non-ingress
  requester's value is discarded, not refused — existing behavior, kept. Turning
  it into an error is a change to the request contract rather than to where
  attribution comes from, and belongs to whoever wants to argue for it.
  Attribution comes from the identity established once at Hello; admission is
  per-connection and attribution does not become per-request.
- **Attribution on locally delivered messages.** The guard that drops a
  self-asserted value governs the local path; this change governs the value
  stamped on an outbound forwarded request. They are separate sites, and only
  the second moves.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `cross-relay-routing`: *Cross-Relay Sender Attribution Forwarding* replaces the
  obligation to omit `on_behalf_of` for an unverified origin with the obligation
  to stamp the identity the origin was admitted under, and records that admission
  is decided in one place.

`runtime-bootstrap` needs no delta. `require-session-credentials` keeps its
meaning exactly — which identities the relay admits at Hello. What changes is a
downstream consequence of an admission the setting already governs, and that
consequence belongs to the capability that specifies forwarding.

`relay-identity` needs no delta either. Its *Sender Attribution Schema* describes
`on_behalf_of` as supplied by an authenticated intermediary and delegates the
cross-relay setting mechanism to `cross-relay-routing`; from the receiver's
position the forwarding relay remains that intermediary whatever it admitted. Its
prohibition on a session without a verified principal populating
`authenticated_identity` is preserved by this change rather than modified by it.

## Impact

No configuration surface changes, so there is nothing for an operator to set,
migrate, or get wrong. Behavior changes on upgrade for any relay running with
`require-session-credentials = false`: its cross-relay messages begin carrying
attribution. That is the intended fix, and it is the only deployment shape that
currently exists here.

A relay that has upgraded and one that has not interoperate in both directions.
The receiving side's handling of a present or absent `on_behalf_of` is unchanged,
so an un-upgraded peer's messages still arrive attributed to the peer principal,
and an upgraded peer's messages arrive attributed to the origin.

The code change is narrow — the stamping condition at the cross-relay forwarding
sites in `send.rs` and `raww.rs`, reading the identity the connection was
admitted under rather than only a store-backed one. The test surface is wider
than the code: the existing relay harness connects as socket-trust throughout,
which is precisely the case this change gives meaning to and which no current
test distinguishes from a credentialed one.
