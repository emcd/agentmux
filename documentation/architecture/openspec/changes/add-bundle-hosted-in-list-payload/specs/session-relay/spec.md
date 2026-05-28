## ADDED Requirements

### Requirement: Bundle Hosted Flag In List Payload

Relay list payloads SHALL include a required field `hosted: bool` on the
canonical `ListedBundle` payload.

`hosted` SHALL be derived on each list request from runtime artifacts and
SHALL be independent of `state`, `startup_health`, and `state_reason_code`.

Hosting predicate:

- For bundles with at least one configured tmux member, `hosted=true` iff
  at least one configured tmux member has an agentmux-owned tmux session
  present on the bundle runtime tmux socket.
- For bundles with zero configured tmux members, `hosted=true` SHALL be
  reported. This matches existing idempotent `bundle up` no-op semantics
  for ACP-only, UI-only, or pubsub-only bundles.

`hosted` SHALL NOT alter or replace existing `state` (`up|down`) or
`startup_health` semantics. `state_reason_code` SHALL continue to describe
`state` and SHALL NOT be suppressed when `hosted=false`.

#### Scenario: Report hosted true when at least one owned tmux session is present

- **WHEN** the bundle has at least one configured tmux member
- **AND** at least one of those tmux members has an agentmux-owned tmux
  session present
- **THEN** relay reports `hosted=true`

#### Scenario: Report hosted false when no configured tmux member is owned

- **WHEN** the bundle has at least one configured tmux member
- **AND** none of those tmux members have an agentmux-owned tmux session
  present
- **THEN** relay reports `hosted=false`

#### Scenario: Report hosted true for ACP-only bundle

- **WHEN** the bundle has zero configured tmux members
- **THEN** relay reports `hosted=true`

#### Scenario: Preserve state and reason fields when hosted false

- **WHEN** relay reports `hosted=false`
- **AND** zero configured sessions are currently ready
- **THEN** relay reports `state=down`
- **AND** `state_reason_code` continues to describe the down condition
