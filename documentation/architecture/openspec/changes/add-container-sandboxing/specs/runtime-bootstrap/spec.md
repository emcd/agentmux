## ADDED Requirements

### Requirement: Container Engine Configuration File

The runtime SHALL support an optional global container configuration artifact at
`<config-root>/containers.toml`. The file SHALL use kebab-case TOML keys and
`format-version = 1`.

When present, `containers.toml` SHALL define the container engine adapter with:

- `[engine].type` (`oci`)
- `[engine].command` (non-empty command name or absolute path, for example
  `docker` or `podman`)

`containers.toml` MAY define sandbox defaults:

- `[defaults].network` (`none` or `host`, default `none`)
- `[defaults].read-only-root` (boolean, default `true`)
- `[defaults].no-new-privileges` (boolean, default `true`)
- `[defaults].capabilities` (string array, default empty)
- `[defaults].user` (string, optional engine user expression)
- `[defaults].relay-socket-target` (absolute container path, default
  `/run/agentmux/relay.sock`)
- `[defaults].tmux-socket-target` (absolute container path, default
  `/run/agentmux/tmux.sock`)
- top-level `[[containers]]` entries defining reusable global container profiles
  with the same profile schema as bundle-local profiles

Malformed TOML, unknown fields, unsupported `format-version`, unsupported engine
type, empty engine command, invalid default values, and invalid shared cache
entries or global container profiles SHALL fail startup and pre-flight
configuration validation with structured validation errors.

#### Scenario: Load OCI engine configuration

- **WHEN** `containers.toml` contains `format-version = 1`
- **AND** `[engine].type = 'oci'`
- **AND** `[engine].command = 'podman'`
- **THEN** runtime configuration accepts the container engine settings

#### Scenario: Reject unsupported container format version

- **WHEN** `containers.toml` contains an unsupported `format-version`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject unknown container configuration field

- **WHEN** `containers.toml` contains an unknown top-level field
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Missing containers.toml is allowed without references

- **WHEN** the configuration root has no `containers.toml`
- **AND** no bundle session references a container profile
- **THEN** relay startup preserves existing unsandboxed session behavior

#### Scenario: Load global container profile

- **WHEN** `containers.toml` defines a top-level `[[containers]]` profile with a
  unique `id`, non-empty `image`, and valid mount entries
- **THEN** runtime configuration accepts the global container profile

### Requirement: Shared Container Cache Configuration

The runtime SHALL support shared read-write cache mounts defined in
`containers.toml` using top-level `[[shared-caches]]` entries. Each shared cache
entry SHALL carry:

- `id`: required non-empty id, unique within `containers.toml`
- `source`: required absolute host path
- `target`: required absolute container path

Shared cache entries SHALL be single read-write bind mounts shared across
containers. They SHALL NOT be translated into per-container volumes. Shared
cache entries are intended for idempotent/content-addressed package caches such
as Cargo registry/git and uv cache directories; exclusive-writer build outputs
such as Cargo `target/` directories SHALL NOT be configured as shared caches by
agentmux defaults.

#### Scenario: Accept shared cache declaration

- **WHEN** `containers.toml` defines a `[[shared-caches]]` entry with a unique
  `id`, an absolute `source`, and an absolute `target`
- **THEN** runtime configuration accepts the shared cache declaration
- **AND** the normalized mount is a shared read-write bind

#### Scenario: Reject relative shared cache path

- **WHEN** a `[[shared-caches]]` entry has a relative `source` or `target`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

### Requirement: Container Engine Capability Check

Before starting a session that references a container profile, the runtime SHALL
verify that the configured engine adapter can honor the profile's required mount,
relay-socket bind, tmux-socket bind, network, capability, user, and
SSH-agent-forwarding contract, and that the referenced image is available
locally. If the adapter cannot prove support for a requested feature, or the
image is not available locally, startup for that session SHALL fail closed with a
structured error and SHALL NOT execute the coder harness unsandboxed.

Engine adapters SHALL NOT rely on generalized host Unix-socket passthrough for
SSH-agent support. SSH-agent support SHALL be an adapter-specific capability
check for the configured engine and platform.

#### Scenario: Fail closed when engine command is unavailable

- **WHEN** a session references a container profile
- **AND** `[engine].command` is not executable
- **THEN** session startup fails with a structured runtime error
- **AND** the coder harness command is not executed on the host

#### Scenario: Fail closed when SSH-agent forwarding is unsupported

- **WHEN** a profile requests `ssh-agent = true`
- **AND** the configured engine adapter cannot provide a working agent socket in
  the container
- **THEN** session startup fails with a structured runtime error
- **AND** the coder harness command is not executed on the host

#### Scenario: Fail closed when image is unavailable locally

- **WHEN** a session references a container profile
- **AND** the profile's configured image is not available to the engine locally
- **THEN** session startup fails with a structured runtime error
- **AND** the runtime does not pull the image on demand
- **AND** the coder harness command is not executed on the host

### Requirement: Container Teardown

The runtime SHALL stop and reap the container for every session that starts a
coder harness inside a container during session-down, watcher-driven bundle
reload/unload eviction, and relay shutdown using bounded shutdown behavior.
Container teardown SHALL account for crashes and SHALL NOT leave the coder
harness running as an orphaned container when agentmux owns the spawned process
lifecycle.

#### Scenario: Stop and reap container on session down

- **WHEN** a running session that references a container profile is brought down
- **THEN** the runtime stops and reaps the associated container within the bounded
  shutdown window
- **AND** the coder harness does not remain running as an orphaned container

#### Scenario: Stop and reap container on relay shutdown

- **WHEN** relay shutdown begins with a running session that references a
  container profile
- **THEN** relay shutdown stops and reaps the associated container within the
  bounded shutdown window
- **AND** shutdown does not fall back to leaving the container running

#### Scenario: Stop and reap container on bundle eviction

- **WHEN** watcher-driven bundle reload or unload evicts a running session that
  references a container profile
- **THEN** the runtime stops and reaps the associated container within the bounded
  shutdown window
- **AND** the coder harness does not remain running as an orphaned container

### Requirement: Container Relay Socket Binding

For every session that references a container profile, the runtime SHALL bind the
resolved host relay Unix socket into the container at the resolved
`relay-socket-target` path. The runtime SHALL inject
`AGENTMUX_RELAY_SOCKET=<relay-socket-target>` into the container environment so
`agentmux` CLI and MCP processes that run inside the sandbox connect to that
in-container socket path. Within the sandbox, `AGENTMUX_RELAY_SOCKET` SHALL take
precedence over state-root-derived relay socket resolution.

Relay socket reachability SHALL NOT depend on `network = 'host'`. A profile with
`network = 'none'` SHALL still be able to reach the local relay through the
explicit socket bind. If the relay socket cannot be bound into the container,
startup for that session SHALL fail closed and SHALL NOT execute the coder
harness unsandboxed.

#### Scenario: Bind relay socket into network-disabled sandbox

- **WHEN** a session references a valid container profile with `network = 'none'`
- **THEN** the engine invocation bind-mounts the resolved host relay socket at
  the configured in-container relay socket path
- **AND** injects `AGENTMUX_RELAY_SOCKET` with that in-container path into the
  container environment

#### Scenario: Resolve relay socket from sandbox environment override

- **WHEN** an `agentmux` CLI or MCP process starts inside a sandbox with
  `AGENTMUX_RELAY_SOCKET` set
- **THEN** client relay connection resolution uses that socket path
- **AND** does not derive the relay socket path from the configured state root

#### Scenario: Fail closed when relay socket cannot be bound

- **WHEN** a session references a container profile
- **AND** the configured engine adapter cannot bind the resolved relay socket into
  the container
- **THEN** session startup fails with a structured runtime error
- **AND** the coder harness command is not executed on the host

### Requirement: Container Tmux Socket Binding

The runtime SHALL bind the resolved per-bundle Tmux Unix socket into every
tmux-backed session container at the resolved `tmux-socket-target` path. If the
host-side session environment contains `TMUX`, the runtime SHALL rewrite only the
socket path component to the in-container tmux socket path and preserve the
remaining tmux server/window suffix.

If the tmux socket cannot be bound into the container, startup for that session
SHALL fail closed and SHALL NOT execute the coder harness unsandboxed.

#### Scenario: Bind tmux socket into tmux-backed sandbox

- **WHEN** a tmux coder-backed session references a valid container profile
- **THEN** the engine invocation bind-mounts the resolved per-bundle tmux socket
  at the configured in-container tmux socket path
- **AND** the containerized harness process receives `TMUX` with the in-container
  socket path and the original tmux suffix

#### Scenario: Fail closed when tmux socket cannot be bound

- **WHEN** a tmux coder-backed session references a container profile
- **AND** the configured engine adapter cannot bind the resolved tmux socket into
  the container
- **THEN** session startup fails with a structured runtime error
- **AND** the coder harness command is not executed on the host
