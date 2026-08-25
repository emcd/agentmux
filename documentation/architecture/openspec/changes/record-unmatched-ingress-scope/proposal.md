## Why

A peer relay's ingress scope can cover nothing on the receiving relay — it names a
namespace that is not hosted, or one that holds no principals. Aggregate namespace
discovery is required to answer that with an ordinary empty result, indistinguishable
from a namespace that does not exist, so the peer cannot probe for existence. That
rule is deliberate and stays.

The cost is that a misconfigured grant is invisible to the operator who issued it.
The receiving relay records `namespace_count: 0` and nothing else: enough to see that
some peer saw nothing, never enough to say which peer or under what grant. A live
cross-relay smoke test hit exactly this. The peer credential had been minted with
scope `all` — a value that is a genuine wildcard in the session-policy vocabulary
(`none`/`self`/`home`/`all`) but, for an ingress scope, is only the name of a
namespace nobody hosts. Discovery returned an empty list, the sibling
concrete-namespace call returned `authorization_forbidden`, and neither relay left a
record naming the scope responsible.

Non-disclosure binds what a relay tells the peer. It does not bind what a relay
records for itself, and the two have been conflated by omission.

## What Changes

- Receiving-side namespace discovery records an inscription when the authenticated
  peer's ingress scope covers no namespace on this relay, carrying the scope and the
  asking principal. Both name things the receiving operator issued, so the record
  discloses nothing the peer did not already present.
- The wire response is unchanged. A scope covering nothing still returns an empty
  success, and the requirement that it be indistinguishable from an absent namespace
  is untouched.
- The aggregate zero-match outcome gains a scenario. The behavior is already
  normative in prose but has no scenario of its own, which is why it has twice been
  read as unspecified and reported as a defect.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `relay-routing-layer`: Cross-Relay Discovery Ingress Filtering gains a normative
  obligation to record an ingress scope that matched nothing, stated so that the
  record is local and the peer-facing result stays unchanged, plus a scenario fixing
  the aggregate zero-match outcome.

## Impact

- `src/relay/handlers/discovery.rs` — `receiving_namespace_discovery` gains the
  requesting principal as an argument and emits the new inscription.
- `tests/integration/session_relay_stream/discovery.rs` — coverage for the new
  record, and for the existing `relay.discovery.namespaces.success` emission, which
  today has no assertions anywhere in the suite.
- No wire, schema, or configuration change. No migration. Operators consuming
  inscriptions gain one new event name.
