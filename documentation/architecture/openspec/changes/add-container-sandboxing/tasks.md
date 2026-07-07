## 1. Configuration Schema

- [ ] 1.1 Add raw and normalized types for `<config-root>/containers.toml` with
  `format-version = 1`, engine command/type fields, default sandbox controls,
  relay socket target path, tmux socket target path, shared cache declarations,
  and global `[[containers]]` profiles.
- [ ] 1.2 Add raw and normalized types for bundle-local `[[containers]]` profiles
  and `[[sessions]].container` references, resolving bundle-local profiles before
  global profiles when ids collide.
- [ ] 1.3 Validate profile ids, duplicate ids, unknown session references,
  missing `containers.toml` when a session references a container, and container
  references on unsupported session types.
- [ ] 1.4 Validate mount entries, including `overlay-slice` parent/slice
  containment, `shared-cache` semantics, mode values, absolute paths, and
  rejection of `~/.ssh` mounts.
- [ ] 1.5 Extend `agentmux check configuration` to parse and validate container
  artifacts and report structured errors for the offending file and field.

## 2. Engine Abstraction

- [ ] 2.1 Add a container engine module that converts normalized profiles into
  engine invocations without leaking Docker/Podman-specific flags into bundle
  configuration.
- [ ] 2.2 Implement OCI command adapters for configured `docker` and `podman`
  commands or fail validation when the configured engine is unsupported.
- [ ] 2.3 Add startup capability checks for required mount, relay-socket bind,
  tmux-socket bind, network, capability, user, local image availability, and
  SSH-agent-forwarding behavior.
- [ ] 2.4 Ensure engine/capability failures fail closed and never fall back to
  unsandboxed execution for a session that references a container profile.
- [ ] 2.5 Account for container stop/reap behavior during session-down,
  watcher-driven bundle reload/unload eviction, relay shutdown, and crash
  recovery without exceeding bounded shutdown contracts.

## 3. Session Startup Integration

- [ ] 3.1 Thread resolved container profile references through bundle
  configuration and relay lifecycle startup.
- [ ] 3.2 Start tmux coder commands through the container engine when the session
  references a profile, including relay-socket binding and resolved environment
  injection into the container process.
- [ ] 3.3 Bind the per-bundle tmux socket for sandboxed tmux-backed sessions and
  rewrite `TMUX` to use the in-container socket path while preserving the tmux
  server/window suffix.
- [ ] 3.4 Start pty coder commands through the container engine when the session
  references a profile by running the container engine command as the host-side
  pty child process, including relay-socket binding and resolved environment
  injection into the container process.
- [ ] 3.5 Start stdio ACP coder commands through the container engine when the
  session references a profile, including relay-socket binding and resolved
  environment injection into the container process.
- [ ] 3.6 Reject container references for ACP HTTP, UI, and pubsub sessions;
  never treat a container profile as an accidental proxy for non-local targets.
- [ ] 3.7 Apply sandbox-injected environment variables after configured session
  environment so `AGENTMUX_RELAY_SOCKET` and SSH-agent values take precedence
  inside the container.
- [ ] 3.8 Update client relay-socket resolution in `src/runtime/paths.rs` and
  CLI/MCP connection paths so `AGENTMUX_RELAY_SOCKET` overrides state-root-derived
  relay socket resolution inside sandboxed processes.

## 4. Tests and Documentation

- [ ] 4.1 Add unit tests for container TOML parsing and validation failures.
- [ ] 4.2 Add tests for relay-socket bind invocation construction and
  `AGENTMUX_RELAY_SOCKET` precedence over state-root-derived client socket
  resolution.
- [ ] 4.3 Add tests for tmux-socket bind invocation construction and `TMUX` socket
  path rewrite for sandboxed tmux-backed sessions.
- [ ] 4.4 Add tests for global `[[containers]]` profile resolution and bundle-local
  profile precedence over global profile ids.
- [ ] 4.5 Add tests for sandbox-injected environment precedence, including
  configured `SSH_AUTH_SOCK` being overridden when `ssh-agent = true`.
- [ ] 4.6 Add tests for working-directory visibility rejection when the session
  directory is outside writable container mounts.
- [ ] 4.7 Add integration tests for bundle session container binding and
  configuration preflight coverage.
- [ ] 4.8 Add engine invocation construction tests that do not require Docker or
  Podman to be installed.
- [ ] 4.9 Add tests for transport-specific stdio no-TTY behavior, pty host-pty
  child execution, and tmux TTY behavior.
- [ ] 4.10 Add tests for container lifecycle teardown during session-down,
  watcher-driven bundle reload/unload eviction, and relay shutdown.
- [ ] 4.11 Update configuration and runtime README documentation with sandbox
  schema examples, SSH-agent behavior, UID/permission caveats, image availability
  expectations, and out-of-scope relay listener notes.
