## MODIFIED Requirements

### Requirement: Runtime Layout

Relay-level artifacts SHALL live at the state-root level:

- `<state_root>/relay.sock`
- `<state_root>/relay.lock`
- `<state_root>/relay.spawn.lock`
- `<state_root>/relay.ready`
- `<state_root>/identity/` — relay-level identity subsystem directory
- `<state_root>/identity/principals.json` — durable principal store

Each bundle SHALL use a dedicated runtime directory for per-bundle artifacts:

- `<state_root>/bundles/<bundle_name>/`
- `<bundle_runtime>/tmux.sock`
- `<bundle_runtime>/sessions/<session>/identity.psk` — session credential file

#### Scenario: Resolve relay-level and per-bundle paths

- **WHEN** runtime paths are resolved
- **THEN** MCP-to-relay IPC uses the single `<state_root>/relay.sock`
- **AND** tmux operations use the per-bundle `<bundle_runtime>/tmux.sock`

#### Scenario: Resolve relay-level principal store

- **WHEN** the relay initializes or processes a Hello
- **THEN** the principal store is located at `<state_root>/identity/principals.json`
- **AND** the `<state_root>/identity/` directory is created if it does not exist

#### Scenario: Resolve session credential file

- **WHEN** a session client reads its identity token
- **THEN** the token is loaded from `<bundle_runtime>/sessions/<session>/identity.psk`
