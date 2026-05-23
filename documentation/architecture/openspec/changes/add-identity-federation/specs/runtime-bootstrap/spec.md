## MODIFIED Requirements

### Requirement: Per-Bundle Runtime Layout

Each bundle SHALL use a dedicated runtime directory:

- `<state_root>/bundles/<bundle_name>/`

The system SHALL use:

- `<bundle_runtime>/tmux.sock`
- `<bundle_runtime>/relay.sock`
- `<bundle_runtime>/identity/` — identity subsystem directory containing the
  durable principal store for the bundle.

#### Scenario: Resolve per-bundle sockets

- **WHEN** runtime paths are resolved for a bundle
- **THEN** tmux operations use that bundle's `tmux.sock`
- **AND** MCP-to-relay IPC uses that bundle's `relay.sock`

#### Scenario: Resolve per-bundle principal store

- **WHEN** the relay initializes for a bundle
- **THEN** the principal store is located under `<bundle_runtime>/identity/`
- **AND** the directory is created if it does not exist
