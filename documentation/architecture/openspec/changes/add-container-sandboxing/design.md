## Context

Agentmux currently relies on coder harness command approval to prevent unsafe
host operations. That approval layer struggles with shell syntax and wrappers,
and it cannot protect unrelated filesystem state if an unsafe command is
approved once. The desired boundary is a named sandbox profile: approve the
profile's mounts and capabilities, then let the harness run with much less
command-level friction.

The operator accepts explicit mount enumeration as the cost of isolation. The
schema therefore optimizes for finite, auditable configuration rather than broad
home-directory convenience mounts.

## Goals / Non-Goals

- Goals: define `containers.toml` global profiles and per-bundle `[[containers]]`
  schema, validate mount/capability profiles, and start local coder harnesses
  inside a referenced sandbox.
- Goals: encode the read-only canonical tree plus read-write owned slice mount
  pattern for source trees and notebooks.
- Goals: encode shared read-write package cache mounts without per-container
  volumes or duplicated downloads.
- Goals: forward SSH-agent access without mounting private key material.
- Non-Goals: TCP relay listener changes, forge adoption, worktree/reference-
  clone provisioning, and generalized host socket passthrough.

## Decisions

- Decision: keep engine-specific details in `<config-root>/containers.toml` and
  keep profile definitions engine-neutral. The first implementation targets
  OCI-style engines invoked by a local command (`docker` or `podman`), but
  profile TOML does not expose Docker-only or Podman-only flags. A future engine
  adapter can reuse the same normalized profile model if it can honor the same
  mount, network, capability, user, and SSH-agent contracts.
- Decision: support reusable global profiles as top-level `[[containers]]`
  entries in `<config-root>/containers.toml` and bundle-local profiles as
  top-level `[[containers]]` entries in `bundles/<bundle-id>.toml`. Coder-backed
  `[[sessions]]` entries opt in with `container = '<profile-id>'`. Bundle-local
  profiles take precedence over global profiles with the same id, letting a
  bundle specialize a shared profile name without changing session references.
- Decision: model source and notebook access with an `overlay-slice` mount type:
  a read-only canonical parent bind mounted first, followed by a read-write owned
  slice bind mounted at its corresponding subpath. Validation requires the owned
  slice to be under the canonical source tree and the container target slice to
  be under the canonical target tree. The name describes the access model, not
  Linux overlayfs: there are no upperdir/lowerdir/whiteout semantics, only nested
  bind mounts.
- Decision: model package caches as explicit `shared-cache` mounts, not
  `overlay-slice` mounts and not per-container volumes. These are single shared
  read-write binds for idempotent/content-addressed caches such as Cargo
  registry/git data or uv cache data. Build outputs such as `target/` and local
  virtualenvs are not separate shared caches; they remain inside the session's
  owned writable tree.
- Decision: bind the resolved relay Unix socket into every sandboxed session at a
  stable container path and inject `AGENTMUX_RELAY_SOCKET` with that in-container
  path. This is the coordination path for `agentmux` CLI/MCP inside the sandbox.
  It is explicit socket reachability, not a reason to require `network = 'host'`.
- Decision: tmux-backed sandbox sessions also bind the per-bundle Tmux Unix
  socket into the container and rewrite the `TMUX` environment socket path to the
  in-container path while preserving the server/window suffix. This keeps tmux
  clients run by the harness pointed at the wrapping tmux server.
- Decision: SSH support forwards an agent socket only. Profiles may request
  `ssh-agent = true`, but the engine adapter must prove it can present a working
  agent socket inside the container for the selected engine/platform or fail
  startup. Profile validation rejects mounts of the operator's `~/.ssh`
  directory or descendants. If configured environment also contains
  `SSH_AUTH_SOCK`, the sandbox-injected agent socket wins so `ssh-agent = true`
  always points at the verified forwarded socket.
- Decision: resolved coder/bundle/session environment must be emitted into the
  container process environment, not only the host-side engine launcher. The
  source environment is the merged session/bundle/coder environment from the
  configurable environment variables contract. Engine-injected variables for
  relay socket and SSH-agent forwarding are applied last.
- Decision: image availability is an operator precondition. Startup must fail
  closed when the configured image is unavailable locally instead of pulling on
  demand, because pull-on-miss adds unbounded first-start latency and weakens the
  fail-fast startup model.
- Decision: this proposal does not solve git stash races or change merge
  workflow. Reference clones and Coordinator-pull merging may be good team
  practice, but they are not agentmux runtime behavior unless agentmux later
  owns checkout provisioning.

## Implementation Considerations

- Container lifecycle must be handled as part of relay/session shutdown, not only
  startup. Session-down, watcher-driven bundle reload/unload eviction, relay
  shutdown, and crash recovery need bounded stop and reap behavior so
  containerized tmux pane commands or ACP children do not leak orphan containers
  or reintroduce shutdown hangs. PID 1 signal handling and zombie reaping, such
  as whether the engine invocation needs an init process, are implementation
  decisions for that work.
- UID and permissions cross both mount and socket boundaries. Files written to
  `overlay-slice` and `shared-cache` read-write mounts use the container-visible
  UID, and the process must also be able to connect to host-owned relay and tmux
  sockets after binding. Docker, rootless Podman, and user namespace remapping
  differ here, so implementation should explicitly choose host-UID execution or
  document the resulting ownership caveat.
- Transport expectations differ by harness type. stdio ACP sessions need
  transparent pipe passthrough with no TTY translation because JSON-RPC framing is
  sensitive to stream changes. pty sessions keep the host-side pty transport and
  terminal emulator on the host; the container engine command is the child
  attached to that host pty, so no additional pty socket bind is introduced. tmux
  sessions need a TTY, but tmux itself remains on the host with the container
  engine command running as the pane command; this proposal does not move the
  tmux server inside the container.

## Risks / Trade-offs

- Engine parity risk: Docker, Podman, and non-Linux hosts differ around socket
  forwarding and bind mounts. Mitigation: validate engine capability at startup
  and fail closed instead of silently running unsandboxed.
- Configuration verbosity: secure isolation requires explicit mounts. Mitigation:
  use `overlay-slice` and `shared-cache` patterns so mount count grows with
  resource classes, not with every reachable project/notebook combination.
- Cache corruption risk: shared writable caches can be damaged by buggy tools.
  Mitigation: only allow explicit `shared-cache` entries; do not recommend or
  auto-configure exclusive-writer paths such as `target/` as shared mounts.
- Cache contention risk: even content-addressed shared caches can have mutable
  indexes or lock files, such as Cargo git indexes, that race under concurrent
  writers. Mitigation: treat shared caches as operator-selected performance
  trade-offs rather than isolation boundaries.
- Compatibility risk: not every session type spawns a local process. Mitigation:
  reject container references on session types that agentmux does not spawn
  locally.
- Boundary risk: `network = 'host'` weakens isolation and is not required for
  agentmux coordination because relay reachability uses the explicit socket bind.
- Host tmux control risk: binding the host tmux socket lets a sandboxed harness
  ask the host tmux server to run commands or target sibling panes. Mitigation:
  treat tmux-socket binding as an explicit compatibility trade-off for tmux-backed
  sessions, not as a general host socket passthrough pattern.
- Host-platform risk: macOS and Docker Desktop can make Unix-socket forwarding
  more fragile than native Linux engines. Mitigation: keep engine capability
  checks explicit and fail closed when the configured engine cannot satisfy the
  socket and mount contract.

## Migration Plan

1. Existing bundles remain valid and unsandboxed when no `container` reference is
   configured.
2. Operators add `containers.toml` with an engine command before adding bundle
   profile references.
3. Operators add one `[[containers]]` profile per bundle lane and move sessions
   onto it one at a time.
4. `agentmux check configuration` reports malformed sandbox artifacts before
   relay startup.

## Open Questions

- None for this proposal. The brainstorm's worktree/reference-clone question is
  intentionally deferred as an external scoping decision, not left open inside
  this change.
