## Context

Two receiving-side discovery handlers answer a peer under its ingress scope.
`receiving_principal_discovery` is asked about one named namespace and can answer
"not yours" with `authorization_forbidden`. `receiving_namespace_discovery` is asked
what the scope covers at all, and when the answer is nothing it returns an empty
success.

That asymmetry reads as an inconsistency and has been filed as one. It is not. A
concrete query and an aggregate query ask different questions, and the empty answer
is required: a namespace-scoped grant covering no principals must be omitted from
discovery, producing the same result as a namespace that does not exist, so a peer
cannot use discovery to probe which namespaces a relay hosts.

What is missing is not an error. It is that the receiving relay keeps no usable
record of the event, so the operator who issued the failing grant cannot find it.

## Goals / Non-Goals

**Goals:**

- Make an ingress scope that covered nothing diagnosable by the operator of the
  relay that issued it.
- Leave the peer-facing result byte-identical.

**Non-Goals:**

- Changing what the peer observes, in any direction.
- Validating a scope value when the peer credential is minted. That would catch the
  originating mistake earlier and is worth doing, but it cannot catch a scope that
  was valid when issued and went stale when a bundle was renamed or removed, and it
  belongs to the credential-management surface rather than to discovery.
- Distinguishing, in the record, a scope that names an unhosted namespace from one
  that names a hosted but empty namespace.

## Decisions

**Record locally rather than fail closed.** The originally filed remedy was to
return `authorization_forbidden` when the scope matches zero candidates. Rejected:
it contradicts the standing requirement that the empty result be indistinguishable
from an absent namespace, and a passing test pins that behavior with a scope naming
an unhosted namespace. Adopting it would have deleted a deliberate non-disclosure
property to gain an operator signal that can be obtained without giving anything up,
because non-disclosure constrains what the relay tells the peer and says nothing
about what it writes to its own inscriptions.

**A distinct event rather than fields on the success record.** The alternative was
to add the scope and principal to `relay.discovery.namespaces.success`. Rejected:
that record is produced by a constructor shared with local discovery, where there is
no ingress scope and no peer, so both fields would be permanently absent on half of
its emissions. A separate event also gives operators something to alert on directly
rather than a predicate over a field of a common event.

**Keep the record factual rather than interpreted.** The relay could consult its
catalog and label the case — scope names nothing hosted, versus scope names something
empty. Rejected for now: the scope and the asking principal are enough to act on,
and deriving a verdict inside a log line adds code whose only purpose is to be read,
and one more thing that can be wrong.

## Risks / Trade-offs

- Logging a scope value → The scope and the principal both name things the receiving
  operator issued and the peer already presented. No credential material is
  involved, and the record never leaves the relay.
- The event fires for a legitimately empty namespace, not only a misconfigured
  scope → Intended. An operator wants to know a peer saw nothing in either case, and
  separating them is explicitly a non-goal above.
- One more event name for inscription consumers to know about → Additive; no
  existing event changes name, shape, or emission condition.

## Migration Plan

None. No wire, schema, or configuration change.
