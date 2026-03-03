## MODIFIED Requirements

### Requirement: Sender Association Resolution

The MCP server SHALL resolve sender association at startup using precedence:

1. explicit CLI `--session-name` when present
2. local override file `session_name` when present
3. auto-discovered sender session

Auto-discovered sender session SHALL use:

- basename of Git worktree top-level directory when running inside Git
- otherwise basename of current working directory

#### Scenario: Resolve sender from worktree basename

- **WHEN** MCP starts inside a Git worktree rooted at
  `/home/me/src/WORKTREES/tmuxmux/relay`
- **AND** no CLI or override sender is provided
- **THEN** sender association resolves to `relay`

#### Scenario: Resolve sender from explicit CLI value

- **WHEN** MCP startup has explicit `--session-name`
- **THEN** sender association is set to that configured session

#### Scenario: Resolve sender from local override file

- **WHEN** CLI sender is absent
- **AND** local override file provides `session_name`
- **THEN** sender association is set to override value

#### Scenario: Reject ambiguous sender association

- **WHEN** sender association candidate matches multiple configured members
- **THEN** MCP startup returns a structured `validation_unknown_sender` error

### Requirement: Relay Auto-Start from MCP

MCP bootstrap SHALL resolve association before attempting relay startup.
After association resolves, MCP bootstrap SHALL attempt to connect to bundle
`relay.sock` first.
If connection fails and auto-start is enabled, MCP SHALL attempt to start the
relay and wait for connectability until timeout.

Default bootstrap values SHALL be:

- `auto_start_relay = true`
- `startup_timeout_ms = 10000`

#### Scenario: Fail startup before relay bootstrap when bundle is unknown

- **WHEN** bundle discovery resolves to an unknown or missing bundle
- **THEN** MCP startup returns structured `validation_unknown_bundle`
- **AND** relay auto-start is not attempted

## ADDED Requirements

### Requirement: Bundle Association Resolution

The MCP server SHALL resolve bundle association at startup using precedence:

1. explicit CLI `--bundle-name` when present
2. local override file `bundle_name` when present
3. auto-discovered bundle

Auto-discovered bundle SHALL use:

- basename of parent directory of Git common-dir when running inside Git
- otherwise basename of current working directory

Resolved bundle SHALL map to a configured bundle definition; otherwise startup
fails with structured `validation_unknown_bundle`.

#### Scenario: Resolve bundle from Git common-dir

- **WHEN** MCP starts in a Git worktree whose Git common-dir is
  `/home/me/src/tmuxmux/.git`
- **AND** no CLI or override bundle is provided
- **THEN** bundle association resolves to `tmuxmux`

#### Scenario: Resolve bundle from local override file

- **WHEN** CLI bundle is absent
- **AND** local override file provides `bundle_name`
- **THEN** bundle association resolves to override value

#### Scenario: Reject unknown bundle association

- **WHEN** resolved bundle has no corresponding configured bundle definition
- **THEN** MCP startup returns structured `validation_unknown_bundle`

#### Scenario: Resolve bundle from explicit CLI value

- **WHEN** MCP startup has explicit `--bundle-name`
- **THEN** bundle association is set to that configured bundle

### Requirement: Local MCP Association Override File

The MCP server SHALL support optional local association overrides in:

- `.auxiliary/configuration/tmuxmux/overrides/mcp.toml`

Supported override fields SHALL include:

- `bundle_name`
- `session_name`

The system MAY support optional config-root override fields for cross-project
bundle coordination.

#### Scenario: Ignore missing override file

- **WHEN** local override file does not exist
- **THEN** startup continues using CLI and auto-discovery resolution

#### Scenario: Reject malformed override file

- **WHEN** local override file exists but has invalid TOML or invalid fields
- **THEN** MCP startup returns a structured bootstrap validation error

### Requirement: Override Directory VCS Posture

The project SHALL Git-ignore the local override directory so overrides can be
used per worktree without leaking to shared commits.

#### Scenario: Ignore local override directory in Git

- **WHEN** repository ignore rules are evaluated
- **THEN** `.auxiliary/configuration/tmuxmux/overrides/` is ignored
