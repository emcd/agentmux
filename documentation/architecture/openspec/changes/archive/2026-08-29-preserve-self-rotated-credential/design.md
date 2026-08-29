## Context

`handle_change_psk` commits the rotated credential, calls
`revoke_streams_for_identity` to force-disconnect connections holding the old
credential, and only then constructs the response carrying the raw PSK. The sweep
matches on `entry.authenticated_identity`, which for a self-rotation is the
requesting connection itself.

The loss is not a write race between two frames on one writer. It is the
response never being written at all:

1. `revoke.notify_one()` fires inside the sweep while `handle_change_psk` is
   still executing on the blocking pool, dispatched by
   `dispatch_on_blocking_pool`.
2. That wake schedules the `serve_connection` select task. The select is
   `biased`, so it polls the frame-loop future first; that future is parked on
   the dispatch `JoinHandle` and returns `Pending`.
3. The `revoke.notified()` arm then resolves and the select returns, dropping the
   frame-loop future.
4. The blocking task runs to completion — `spawn_blocking` is not cancelled — but
   its `RelayResponse::ChangePsk` is discarded along with the dropped
   `JoinHandle`. The response write never executes.

The remaining work between the notify and the handler's return (trusted-host
fan-out, timestamp formatting, an inscription file append) is what gives the
frame loop its rare win: 2 runs in 50, where the blocking task completed before
the executor reached the select task.

Socket-trust connections carry no `authenticated_identity` and are never matched
by the sweep, so only a requester provisioned with a real PSK can reach this.

## Goals / Non-Goals

**Goals:**

- A principal rotating its own credential receives the rotated PSK.
- Rotation's teardown behavior keeps an unambiguous normative home.
- Third-party revocation on rotation is unchanged and stays pinned by a test.

**Non-Goals:**

- Reworking dispatch so a handler can order a response ahead of a side effect.
  That would fix this class generally rather than this instance, but it means
  threading a deferred side effect out through `dispatch_on_blocking_pool`'s
  return type, which every relay handler shares.
- Forbidding self-rotation. Rejected on the merits below.
- Any change to `expires_at` handling, the drop command, or the sweep's own
  contract.

## Decisions

### Exclude the requester rather than forbid self-rotation

Rotating one's own credential is a legitimate and probably common operator
action. Forbidding it — the shape the drop command uses for self-drop — trades a
credential-loss bug for a capability gap, and the two cases are not alike: a
dropped principal is meant to stop working, while a rotated one is meant to keep
working with a secret the operator must actually receive.

Refusal would also require a new error code, and therefore an edit to the
**Error Object Contract** requirement, which neighbouring identity-administration
work has been modifying. Exclusion touches only the requirement that owns
rotation.

### Excluding the requester cannot spare a third party

This is what makes the carve-out safe rather than a hole, and it is a property of
the registry rather than of the sweep: `stream_registry` keys entries by
`principal_id`, one entry per principal. A second connection claiming a live
identity is refused as an identity-claim conflict rather than admitted alongside
the first.

So for a self-rotation the requester's own connection is the *only* entry the
sweep can match. Skipping it cannot leave some other session alive holding the
prior credential, because no such session can exist. Were the registry ever
re-keyed to admit multiple connections per principal, this carve-out would need
to narrow from "the requesting principal" to "the requesting connection", and
that is the condition to watch.

### The policy lives at the call site, not in the sweep

`revoke_streams_for_identity` keeps its contract — tear down every live stream
whose verified identity matches. The handler is what knows who asked. Pushing a
requester parameter into the sweep would give a general eviction primitive a
notion of "the caller" that only one of its callers has.

### Rotation is not revocation, and the spec should say so

`relay-identity`'s **Revocation and Expiry Enforcement** requirement speaks
throughout of a principal being *revoked or expiring* — a principal ceasing to be
valid. A rotation replaces a credential; the principal persists and is expected
to reconnect with the new one. Rotation only ever read as covered because it was
historically the requirement's sole caller, before a true principal-removal
operation existed.

Scoping that requirement to drop and expiry would leave rotation's teardown
behavior with no normative home at all, so the same change moves that contract
onto **MCP Change Tool**, which owns rotation. Nothing is deleted without a
statement of what replaces it: the typed `runtime_identity_revoked` frame ahead
of close, and the `identity.revoked` fan-out to in-scope trusted hosts, are both
restated there.

### Trusted-host fan-out is not excluded

Only the connection teardown is skipped for a self-rotation. Watching hosts still
receive `identity.revoked`, because the credential did change and a host holding
a cached view must drop it. The two mechanisms already have distinct purposes;
the carve-out applies to exactly one of them.

## Risks / Trade-offs

- **A self-rotating connection stays alive on a credential that was just
  rotated** → It is the connection that requested the rotation and is being
  handed the new secret; there is no privilege it gains that it did not already
  have. An attacker positioned to reach this already holds the old credential
  *and* an `all`-scoped `change.psk` grant.

- **A test asserting "the requester is not torn down" is an absence assertion,
  which can pass because the whole mechanism broke** → The same delta pins
  third-party revocation as a positive control, so a sweep that stopped revoking
  anything fails that test rather than silently satisfying this one.

- **The registry-keying argument is load-bearing but lives one module away from
  the carve-out** → It is stated in the requirement rationale and at the call
  site, not left to a commit message, so a future change that re-keys the
  registry has a reason to find it.
