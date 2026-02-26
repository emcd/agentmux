## ADDED Requirements

### Requirement: XDG Configuration Root

The system SHALL resolve the configuration root using:

- `$XDG_CONFIG_HOME/tmuxmux` when `XDG_CONFIG_HOME` is set and non-empty
- `~/.config/tmuxmux` otherwise

#### Scenario: Resolve config root from XDG variable

- **WHEN** `XDG_CONFIG_HOME` is set to a non-empty value
- **THEN** configuration root resolves under that directory

#### Scenario: Resolve config root from fallback

- **WHEN** `XDG_CONFIG_HOME` is unset or empty
- **THEN** configuration root resolves to `~/.config/tmuxmux`

### Requirement: XDG State Root

The system SHALL resolve the state root using:

- `$XDG_STATE_HOME/tmuxmux` when `XDG_STATE_HOME` is set and non-empty
- `~/.local/state/tmuxmux` otherwise

#### Scenario: Resolve state root from XDG variable

- **WHEN** `XDG_STATE_HOME` is set to a non-empty value
- **THEN** state root resolves under that directory

#### Scenario: Resolve state root from fallback

- **WHEN** `XDG_STATE_HOME` is unset or empty
- **THEN** state root resolves to `~/.local/state/tmuxmux`

### Requirement: Debug Repository-Local State Override

Debug builds SHALL support an optional repository-local state override to
isolate development runtime data from deployed runtime state.

#### Scenario: Use repository-local override in debug build

- **WHEN** runtime is debug/development mode
- **AND** repository-local override is configured
- **THEN** state root resolves to repository-local override path

#### Scenario: Ignore repository-local override in non-debug build

- **WHEN** runtime is not debug/development mode
- **THEN** state root resolution follows XDG state rules

### Requirement: Per-Bundle Runtime Layout

Each bundle SHALL use a dedicated runtime directory:

- `<state_root>/bundles/<bundle_name>/`

The system SHALL use:

- `<bundle_runtime>/tmux.sock`
- `<bundle_runtime>/relay.sock`

#### Scenario: Resolve per-bundle sockets

- **WHEN** runtime paths are resolved for a bundle
- **THEN** tmux operations use that bundle's `tmux.sock`
- **AND** MCP-to-relay IPC uses that bundle's `relay.sock`

### Requirement: Relay Auto-Start from MCP

MCP bootstrap SHALL attempt to connect to bundle `relay.sock` first.
If connection fails and auto-start is enabled, MCP SHALL attempt to start the
relay and wait for connectability until timeout.

Default bootstrap values SHALL be:

- `auto_start_relay = true`
- `startup_timeout_ms = 10000`

#### Scenario: Use running relay when available

- **WHEN** `relay.sock` is connectable during MCP bootstrap
- **THEN** MCP continues without spawning a new relay process

#### Scenario: Auto-start relay when unavailable

- **WHEN** `relay.sock` is not connectable and auto-start is enabled
- **THEN** MCP executes relay bootstrap flow
- **AND** waits up to configured timeout for relay connectivity

#### Scenario: Fail fast when auto-start disabled

- **WHEN** `relay.sock` is not connectable and auto-start is disabled
- **THEN** MCP returns a structured bootstrap error

### Requirement: Spawn Coordination and Stale Socket Handling

Relay startup SHALL use lock-based spawn coordination so exactly one contender
spawns relay while others wait for socket readiness.

#### Scenario: Single spawner under contention

- **WHEN** multiple MCP servers bootstrap concurrently for one bundle
- **THEN** only one process performs relay spawn
- **AND** other processes wait for relay socket connectability

#### Scenario: Remove stale relay socket before spawn

- **WHEN** relay socket exists but no live relay process holds runtime lock
- **THEN** bootstrap removes the stale socket before spawning relay

### Requirement: Sender Association Resolution

The MCP server SHALL resolve sender association at startup using:

- explicit configured sender session when present
- otherwise working-directory match against configured bundle member
  `working_directory`

#### Scenario: Resolve sender from explicit configuration

- **WHEN** MCP startup has explicit sender session configuration
- **THEN** sender association is set to that configured session

#### Scenario: Resolve sender from working directory

- **WHEN** no explicit sender session is configured
- **AND** exactly one bundle member matches MCP working directory
- **THEN** sender association is set to that matching member

#### Scenario: Reject ambiguous working-directory matches

- **WHEN** multiple bundle members match MCP working directory
- **THEN** MCP startup returns a structured bootstrap error

### Requirement: Runtime Security Posture

Runtime artifacts SHALL remain inside same-user ownership and restrictive local
permissions.

#### Scenario: Create restrictive runtime directory

- **WHEN** system creates bundle runtime directory
- **THEN** directory mode is `0700`

#### Scenario: Reject foreign-owned runtime artifact

- **WHEN** an existing runtime socket or lock file is not owned by current user
- **THEN** bootstrap returns a structured security error
