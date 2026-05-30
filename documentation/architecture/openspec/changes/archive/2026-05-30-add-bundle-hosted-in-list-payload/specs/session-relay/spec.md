## ADDED Requirements

### Requirement: Per-Session Readiness In List Payload

Relay list payloads SHALL include a required field `ready: bool` on each
`ListedSession` entry.

`ready` SHALL be derived on each list request from a per-transport
readiness predicate:

- tmux member: `ready=true` iff relay resolves an active pane target for
  the configured tmux session.
- ACP member: `ready=true` iff the shared per-target ACP worker reports
  ready state for the configured ACP session.
- ui or pubsub member: `ready=false` always (no implemented startup path).

Per-session readiness SHALL be the single source of truth used to derive
the bundle-level aggregates (`state`, `startup_health`, `hosted`) within
the same list request.

#### Scenario: Report ready true for tmux session with resolvable pane

- **WHEN** a configured tmux member has a resolvable active pane target
- **THEN** the listed session entry reports `ready=true`

#### Scenario: Report ready false for tmux session without resolvable pane

- **WHEN** a configured tmux member has no resolvable active pane target
- **THEN** the listed session entry reports `ready=false`

#### Scenario: Report ready true for ACP session with ready worker

- **WHEN** a configured ACP member has a ready shared worker
- **THEN** the listed session entry reports `ready=true`

#### Scenario: Report ready false for ACP session without ready worker

- **WHEN** a configured ACP member has no ready shared worker
- **THEN** the listed session entry reports `ready=false`

#### Scenario: Report ready false for ui or pubsub member

- **WHEN** a configured member is of transport ui or pubsub
- **THEN** the listed session entry reports `ready=false`

### Requirement: Bundle Hosted Flag In List Payload

Relay list payloads SHALL include a required field `hosted: bool` on the
canonical `ListedBundle` payload.

`hosted` SHALL be derived on each list request from per-session readiness
and SHALL be independent of `state`, `startup_health`, and
`state_reason_code`.

Hosting predicate:

- `hosted=true` iff at least one configured member is ready.
- `hosted=false` otherwise, including the empty-bundle case
  (zero configured members).

`hosted` SHALL NOT alter or replace existing `state` (`up|down`) or
`startup_health` semantics. `state_reason_code` SHALL continue to describe
`state` and SHALL NOT be suppressed when `hosted=false`.

#### Scenario: Report hosted true when at least one member is ready

- **WHEN** at least one configured bundle member is ready
- **THEN** relay reports `hosted=true`

#### Scenario: Report hosted false when no configured member is ready

- **WHEN** zero configured bundle members are ready
- **THEN** relay reports `hosted=false`

#### Scenario: Report hosted false for ACP-only bundle with no ready worker

- **WHEN** the bundle has only configured ACP members
- **AND** none of those ACP members report ready
- **THEN** relay reports `hosted=false`

#### Scenario: Preserve state and reason fields when hosted false

- **WHEN** relay reports `hosted=false`
- **AND** zero configured sessions are currently ready
- **THEN** relay reports `state=down`
- **AND** `state_reason_code` continues to describe the down condition
