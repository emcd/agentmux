## MODIFIED Requirements

### Requirement: Stream Reconnect Behavior

On stream disconnect, clients SHALL attempt reconnect with same identity and
repeat `hello` registration.

Reconnect failures SHALL be surfaced as `relay_unavailable` errors in existing
caller-facing paths.

Reconnect logic SHALL preserve identity-ownership hardening behavior:

- reconnect `hello` claim is accepted when no conflicting live owner exists for
  `(bundle_name, session_id)`, or when prior owner is already hard-dead per
  relay evidence contract;
- conflicting live-owner claims are rejected with
  `runtime_identity_claim_conflict`.

#### Scenario: Re-register identity after reconnect without live conflict

- **WHEN** client stream reconnect succeeds after disconnect
- **AND** no conflicting live owner exists for that identity
- **THEN** client sends `hello` with same identity
- **AND** relay accepts identity binding

#### Scenario: Reject reconnect claim while prior owner remains live

- **WHEN** reconnect attempt sends `hello` for identity with conflicting live
  owner
- **THEN** relay rejects claim with `runtime_identity_claim_conflict`

#### Scenario: Surface relay unavailable on reconnect failure

- **WHEN** reconnect attempt fails to establish stream
- **THEN** client surfaces `relay_unavailable` in caller-facing error path
