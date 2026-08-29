## Why

A principal that rotates its own PSK almost never receives the rotated
credential. The relay commits the new credential hash, then tears down the very
connection it owes the response to, and the only copy of the new secret is
discarded with it. Measured over 50 runs of a reproduction probe, the credential
is lost in 48. With the Response destination the secret is stored nowhere else,
so the principal is locked out of a relay that has already published its new
hash — recoverable only by hand-editing `principals.json`.

This is invisible to the existing test suite for a specific reason worth
recording: the revocation sweep matches on `authenticated_identity`, which
socket-trust connections do not carry, and the entire relay-stream harness
connects as socket-trust. A defect reproducible 96% of the time sat behind a
harness that structurally cannot reach it.

## What Changes

- A principal rotating its own credential keeps its requesting connection. The
  revocation sweep excludes the requester, so the response carrying the new PSK
  is written rather than discarded.
- Rotation's teardown contract moves to the requirement that owns rotation.
  Today it is implied by a revocation requirement whose text speaks only of a
  principal being revoked or expiring, which a rotation is not: the credential is
  replaced and the principal persists, expected to reconnect.
- Rotation continues to revoke a *third party's* session holding the prior
  credential, and continues to emit `identity.revoked` to in-scope trusted-host
  streams — including for a self-rotation, since watching hosts must drop cached
  views of a credential that changed either way.

Not breaking: no wire field, error code, or request shape changes. The affected
operation currently fails; it will succeed.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `relay-identity`: **Revocation and Expiry Enforcement** is scoped to a
  principal ceasing to be valid — drop and expiry — rather than left ambiguous
  about credential rotation, which it reads as covering only because rotation was
  historically its sole caller.
- `mcp-tool-surface`: **MCP Change Tool** gains rotation's own teardown contract,
  including the requester carve-out, so the behavior removed from
  `relay-identity`'s scope keeps a normative home instead of falling into a gap.

## Impact

- `src/relay/handlers/identity.rs` — `handle_change_psk` skips the revocation
  sweep when the requester is rotating its own principal.
- `tests/unit/relay_stream/identity/psk_lifecycle.rs` — a self-rotation test that
  provisions the requester with a real PSK, since a socket-trust requester cannot
  reach the defect, plus a positive control pinning third-party revocation.
- No change to `src/relay/stream/eviction.rs`: the sweep's own contract is
  unchanged, and the policy about who is swept belongs at the call site that
  knows the requester.
- No wire, storage, or configuration compatibility impact.
