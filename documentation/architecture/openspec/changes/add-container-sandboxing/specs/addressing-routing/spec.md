## ADDED Requirements

### Requirement: Bundle Container Profile Configuration

Per-bundle TOML configuration SHALL support sandbox profiles using top-level
`[[containers]]` entries. Each profile SHALL carry:

- `id`: required non-empty id, unique within the bundle file
- `image`: required non-empty container image reference
- `network`: optional `none` or `host`, defaulting from `containers.toml`
- `read-only-root`: optional boolean, defaulting from `containers.toml`
- `no-new-privileges`: optional boolean, defaulting from `containers.toml`
- `capabilities`: optional string array, defaulting from `containers.toml`
- `user`: optional engine user expression, defaulting from `containers.toml`
- `ssh-agent`: optional boolean, default `false`
- `[[containers.mounts]]` entries defining explicit visible host paths

Container profile ids are bundle-local by default and MAY resolve to a global id
in `<config-root>/containers.toml` when no bundle-local match exists. Malformed
profiles, duplicate profile ids, unknown fields, invalid scalar values, and
invalid mount entries SHALL fail startup and pre-flight configuration validation
with structured validation errors.

Session `container` references SHALL resolve first against bundle-local
`[[containers]]` profiles, then against global `[[containers]]` profiles from
`<config-root>/containers.toml`. A bundle-local profile id SHALL override a
global profile with the same id for that bundle only.

#### Scenario: Load bundle container profile

- **WHEN** a bundle file defines a `[[containers]]` profile with a unique `id`,
  non-empty `image`, and valid mount entries
- **THEN** bundle configuration loads the profile successfully

#### Scenario: Reject duplicate container profile id

- **WHEN** a bundle file defines two `[[containers]]` profiles with the same `id`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject invalid profile network mode

- **WHEN** a `[[containers]]` profile sets `network` to a value other than `none`
  or `host`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Resolve global container profile reference

- **WHEN** a session declares `container = 'shared'`
- **AND** the same bundle file has no `[[containers]]` profile with that `id`
- **AND** `containers.toml` has a global `[[containers]]` profile with
  `id = 'shared'`
- **THEN** bundle configuration resolves the session to the global profile

#### Scenario: Bundle-local profile overrides global profile

- **WHEN** a session declares `container = 'shared'`
- **AND** the same bundle file has a `[[containers]]` profile with `id = 'shared'`
- **AND** `containers.toml` also has a global `[[containers]]` profile with
  `id = 'shared'`
- **THEN** bundle configuration resolves the session to the bundle-local profile


### Requirement: Container Mount Profile Semantics

Container profile mount entries SHALL use a required `type` discriminator:

- `bind`: an explicit bind mount with required absolute `source`, required
  absolute `target`, and required `mode` (`ro` or `rw`)
- `overlay-slice`: a read-only canonical tree plus read-write owned slice, with
  required absolute `source`, `target`, `writable-source`, and
  `writable-target`
- `shared-cache`: a reference to a `containers.toml` `[[shared-caches]].id`,
  with required `id`

For `overlay-slice`, the runtime SHALL mount `source` read-only at `target`, then
mount `writable-source` read-write at `writable-target`. Validation SHALL require
`writable-source` to be contained by `source`, and `writable-target` to be
contained by `target`. This pattern applies to source trees and notebook roots:
the canonical tree is visible read-only, while the session's own lane/notebook
slice is writable at the corresponding subpath.

For `shared-cache`, the runtime SHALL mount the referenced shared cache as a
single shared read-write bind mount. It SHALL NOT synthesize per-container
volumes.

The runtime SHALL reject any mount whose source resolves to the operator's
`~/.ssh` directory or a descendant of it.

`overlay-slice` SHALL mean nested bind mounts only. It SHALL NOT imply overlayfs
copy-on-write semantics such as upperdir, lowerdir, or whiteouts.

#### Scenario: Layer owned source slice over read-only source tree

- **WHEN** a profile declares an `overlay-slice` mount with `source` set to a
  canonical source root
- **AND** `writable-source` is a lane checkout under that source root
- **THEN** the engine invocation mounts the canonical source root read-only
- **AND** mounts the lane checkout read-write at the corresponding target path

#### Scenario: Layer own notebook over read-only notebook root

- **WHEN** a profile declares an `overlay-slice` mount for `NB_DIR`
- **AND** `writable-source` is the session's own notebook subdirectory
- **THEN** the container can read the notebook root
- **AND** can write only the owned notebook subdirectory through that mount

#### Scenario: Mount shared package cache once as read-write

- **WHEN** a profile references a `shared-cache` mount
- **THEN** the engine invocation mounts the referenced cache read-write
- **AND** does not create a per-container volume for that cache

#### Scenario: Reject overlay slice outside canonical tree

- **WHEN** an `overlay-slice` mount has `writable-source` outside `source`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject private SSH key mount

- **WHEN** any profile mount source resolves to `~/.ssh` or a descendant path
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact


### Requirement: Session Container Binding

Coder-backed `[[sessions]]` entries SHALL support an optional container profile
reference using `container = '<profile-id>'`. When a session references a
profile, the profile SHALL resolve to a bundle-local profile or to a global
profile from `<config-root>/containers.toml` using the profile resolution order.
`<config-root>/containers.toml` SHALL be present and valid for any session that
references a container profile. If neither scope defines the referenced profile,
startup and pre-flight configuration validation SHALL fail with a structured
validation error.

Container binding SHALL be supported only for session targets that agentmux
starts as local processes:

- tmux coder sessions
- pty coder sessions
- stdio ACP coder sessions

Container binding SHALL be rejected for ACP HTTP sessions, UI sessions, and
pubsub sessions because agentmux does not spawn their target process locally.

If a session references a valid container profile, relay lifecycle SHALL start
the coder harness inside that profile. The session's configured working
directory SHALL resolve to a path visible inside the container profile; otherwise
startup SHALL fail closed. A referenced profile SHALL never fall back to host
execution after validation, engine, or startup failure.

A working directory is visible when it equals or is contained by an
`overlay-slice` `writable-target`, or when it equals or is contained by the
`target` of a `bind` mount whose `mode` is `rw`. Read-only binds and
shared-cache mounts SHALL NOT make a session working directory visible.

Sessions that omit `container` SHALL retain existing unsandboxed startup
behavior.

#### Scenario: Start tmux session inside referenced container

- **WHEN** a tmux coder-backed session references a valid container profile
- **THEN** relay lifecycle starts the tmux coder command through the container
  engine
- **AND** does not execute the coder command directly on the host

#### Scenario: Start stdio ACP session inside referenced container

- **WHEN** a stdio ACP coder-backed session references a valid container profile
- **THEN** relay lifecycle starts the ACP command through the container engine
- **AND** does not execute the ACP command directly on the host

#### Scenario: Start pty session inside referenced container

- **WHEN** a pty coder-backed session references a valid container profile
- **THEN** relay lifecycle keeps the host-side pty transport and terminal
  emulator on the host
- **AND** starts the container engine command as the pty child process
- **AND** does not execute the pty command directly on the host

#### Scenario: Reject unknown container reference

- **WHEN** a session declares `container = 'missing'`
- **AND** the same bundle file has no `[[containers]]` profile with that `id`
- **AND** `containers.toml` has no global `[[containers]]` profile with that `id`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject container binding for ACP HTTP session

- **WHEN** an ACP HTTP session declares `container = '<profile-id>'`
- **THEN** relay startup fails with a structured validation error
- **AND** the runtime does not treat the container profile as a network proxy

#### Scenario: Reject working directory outside visible mounts

- **WHEN** a session references a valid container profile
- **AND** the session's configured working directory does not resolve to a path
  visible through that profile's mounts
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Preserve unsandboxed behavior when omitted

- **WHEN** a coder-backed session omits `container`
- **THEN** relay lifecycle starts the session using existing host execution
  behavior
