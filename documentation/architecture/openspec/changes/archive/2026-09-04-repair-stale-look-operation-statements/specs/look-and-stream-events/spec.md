## MODIFIED Requirements

### Requirement: Relay Look Operation

The system SHALL provide a relay-level read-only inspection operation:
`look`.

`look` request fields SHALL include:

- `requester_session` (required)
- `target_session` (required) — a fully-qualified principal id, either
  `<session>@<bundle>` naming the requester's own or a peer bundle, or a
  relay-wide `<id>@GLOBAL` principal. A bare target is rejected at the shared
  resolution stage with `validation_unqualified_target`; filling in the
  namespace is the calling surface's job (see Suffix-Based Target Routing)
- `lines` (optional)
- `offset` (optional; default `0`) — for ACP targets, pages the entry
  window backward from the newest end; for tmux targets only `0` is valid

The relay SHALL resolve the look target's hosting bundle from the
`target_session` suffix and SHALL capture the snapshot from that bundle's
runtime context.

A relay-wide (`@GLOBAL`) target names no bundle to capture from, so it SHALL be
resolved as relay-wide and rejected by the look capability check rather than as
an unknown bundle: `validation_unknown_target` when the principal is not
registered, and `validation_unsupported_operation` when it is registered and its
transport carries `can_be_looked = false` (see Transport Capability Contract).

Authorization posture for `look` SHALL be:

- built-in default scope `home`, applied when no policy preset resolves for the
  requester (see Policy Preset Source)
- self-inspection (requester equals target) is always permitted; this shortcut
  applies to same-bundle look only
- same-bundle inspection of a different session requires `home`
- cross-bundle inspection of a peer bundle's session requires `all`,
  evaluated against the requester's own (dispatch) bundle policy; `home`
  confers no authority beyond the requester's own bundle

#### Scenario: Reject bare look target

- **WHEN** look request target carries no `@<namespace>` suffix
- **THEN** relay rejects request with `validation_unqualified_target`
- **AND** relay does not resolve the target against the requester's bound bundle

#### Scenario: Reject relay-wide look target via capability check

- **WHEN** look request target is a registered `<id>@GLOBAL` principal whose
  transport carries `can_be_looked = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not return `validation_unknown_bundle`

#### Scenario: Reject unregistered relay-wide look target

- **WHEN** look request target is an `<id>@GLOBAL` principal that is not
  registered
- **THEN** relay returns `validation_unknown_target`

#### Scenario: Resolve peer bundle from target suffix

- **WHEN** look request target is `<session>@<peer-bundle>` where
  `<peer-bundle>` differs from the requester's dispatch bundle
- **AND** `<peer-bundle>` is configured on the relay and `<session>` is a member
- **AND** requester policy has `look = "all"`
- **THEN** relay captures the snapshot from `<peer-bundle>`'s runtime and
  returns `target_session = <session>@<peer-bundle>` with `requester_session`
  echoed in its own dispatch bundle

#### Scenario: Reject unknown peer bundle

- **WHEN** look request target names a bundle that is not configured on this
  relay
- **THEN** relay rejects request with `validation_unknown_bundle`

#### Scenario: Reject unknown peer session

- **WHEN** look request target names a configured peer bundle but a session that
  is not a member of that bundle
- **THEN** relay rejects request with `validation_unknown_target`

#### Scenario: Deny cross-bundle look under home scope

- **WHEN** requester targets a session in a peer bundle
- **AND** requester policy has `look = "home"` or narrower
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** requester and target are different sessions in same bundle
- **AND** requester policy has `look = "self"`
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Reject nonzero offset on tmux target

- **WHEN** look request targets a tmux session
- **AND** `offset` is present and not equal to `0`
- **THEN** relay rejects request with `validation_offset_unsupported`

#### Scenario: Accept zero offset on tmux target

- **WHEN** look request targets a tmux session
- **AND** `offset` is omitted or equal to `0`
- **THEN** relay accepts request and proceeds with the look operation
