## MODIFIED Requirements

### Requirement: Relay Look Operation

The system SHALL provide a relay-level read-only inspection operation:
`look`.

`look` request fields SHALL include:

- `requester_session` (required)
- `target_session` (required) — MAY be a bare session id (resolved within the
  requester's bound/dispatch bundle) or a peer-qualified `<session>@<bundle>`
  id that selects a peer bundle by suffix (consistent with `Send` target
  routing)
- `lines` (optional)
- `offset` (optional; default `0`) — for ACP targets, pages the entry
  window backward from the newest end; for tmux targets only `0` is valid
- `bundle_name` (optional, redundant) — treated as the dispatch-bundle echo; it
  SHALL NOT select or reject a peer bundle. The peer bundle is derived solely
  from the `target_session` suffix

The relay SHALL resolve the look target's hosting bundle from the
`target_session` suffix and SHALL capture the snapshot from that bundle's
runtime context.

Authorization posture for `look` SHALL be:

- default scope `self`
- self-inspection (requester equals target) is always permitted; this shortcut
  applies to same-bundle look only
- same-bundle inspection of a different session requires `all:home`
- cross-bundle inspection of a peer bundle's session requires `all:all`,
  evaluated against the requester's own (dispatch) bundle policy; `all:home`
  confers no authority beyond the requester's own bundle

#### Scenario: Resolve bundle from associated runtime context

- **WHEN** look request target is bare and omits `bundle_name`
- **THEN** relay resolves the target within the associated (bound) bundle

#### Scenario: Accept redundant matching bundle name

- **WHEN** look request includes `bundle_name` matching the associated runtime
  context
- **THEN** relay accepts request and proceeds with the look operation

#### Scenario: Resolve peer bundle from target suffix

- **WHEN** look request target is `<session>@<peer-bundle>` where
  `<peer-bundle>` differs from the requester's dispatch bundle
- **AND** `<peer-bundle>` is configured on the relay and `<session>` is a member
- **AND** requester policy has `look = "all:all"`
- **THEN** relay captures the snapshot from `<peer-bundle>`'s runtime and
  returns `bundle_name = <peer-bundle>` with the requester echoed in its own
  dispatch bundle

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
- **AND** requester policy has `look = "all:home"` or narrower
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
