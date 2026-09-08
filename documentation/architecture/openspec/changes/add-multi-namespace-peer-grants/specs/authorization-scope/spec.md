## ADDED Requirements

### Requirement: Peer Scope Administration Authorization

The relay SHALL authorize `change scope` using the named action `scope` in the
existing `change` policy-control map. Only `change.scope` at `all` SHALL permit
the relay-wide action. Missing, `none`, `self`, and `home` SHALL deny with
`authorization_forbidden` identifying capability `change.scope`.

This gate SHALL apply to widening, narrowing, clearing and no-op replacement.
`new.peer`, `change.psk`, `drop.peer`, and a peer ingress wildcard SHALL NOT
confer scope-update authority. Scope-update authority SHALL NOT confer those
other administrative permissions. CLI/MCP SHALL pass relay authorization
results through rather than perform shadow policy evaluation.

The starter operator preset SHALL explicitly grant `change.scope='all'` under
its change controls. Existing configuration SHALL NOT acquire that permission
implicitly or be rewritten; the control remains optional and deny-by-default.

#### Scenario: Rotation-only caller cannot change scope

- **WHEN** a caller holds `change.psk=all` but no `change.scope=all`
- **THEN** scope replacement is denied without changing the record
- **AND** credential rotation still preserves the current scope

#### Scenario: Explicit scope control permits replacement only

- **WHEN** a caller holds `change.scope=all` but no credential admin controls
- **THEN** it may replace or clear a registered peer's grant
- **AND** it gains no registration, rotation or drop permission

#### Scenario: Narrow scope tiers do not authorize relay-wide administration

- **WHEN** a caller's `change.scope` is missing, none, self, or home
- **THEN** every scope-update request is denied, including a no-op or narrowing

#### Scenario: Starter grant is explicit without changing existing policies

- **WHEN** a new deployment hydrates the starter operator preset
- **THEN** that preset explicitly includes `change.scope='all'`
- **AND** an existing policy file lacking the action remains unchanged and denied
