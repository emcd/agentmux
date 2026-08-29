# Change: Add Container Sandboxing for Coder Sessions

## Why

Command-string approval is brittle for real coder harness usage: pipelines,
substitutions, wrappers such as `time` and `timeout`, and tool-generated shell
forms are hard to classify safely. Moving the approval boundary to explicit
container mount and capability profiles lets operators audit a smaller, more
stable surface while letting coder harnesses run with fewer command-level
interruptions.

Containers also make accidental host filesystem damage and process interference
structurally harder: a failed shell policy check is no longer the only boundary
between an agent and unrelated worktrees, notebooks, caches, or credentials.

## What Changes

- Add a global `<config-root>/containers.toml` artifact for container engine
  selection, shared sandbox defaults, shared caches, and reusable global
  `[[containers]]` profiles.
- Add per-bundle `[[containers]]` mount/capability profiles; coder-backed
  sessions can reference either a bundle-local or global profile by id.
- Add explicit mount-profile semantics for read-only canonical trees with a
  layered read-write owned slice, shared read-write package caches, and ordinary
  read-only/read-write bind mounts.
- Add first-class relay socket binding so sandboxed coder harnesses can still
  reach local agentmux CLI/MCP coordination without relying on host networking.
- Add first-class Tmux socket binding for sandboxed tmux-backed sessions so tmux
  clients inside the container can reach the wrapping tmux server.
- Add SSH-agent forwarding as an explicit profile option that forwards
  `SSH_AUTH_SOCK` only and rejects `~/.ssh` mounts.
- Inject the resolved coder/bundle/session environment from the configurable
  environment variables contract into the containerized harness process, then
  apply sandbox-injected `AGENTMUX_RELAY_SOCKET` and `SSH_AUTH_SOCK` overrides.
- Start local coder harness processes inside the referenced sandbox profile for
  tmux, pty, and stdio ACP sessions.
- Keep non-referenced sessions unsandboxed so operators can migrate bundle by
  bundle.

## Out Of Scope

- TCP/IP relay listener support. The relay listener remains Unix-socket based in
  this proposal.
- Full forge adoption for merge control.
- Worktree-to-reference-clone migration and Coordinator-pull merge workflow.
  That remains an operational/team-practice scoping decision unless a future
  change makes agentmux responsible for checkout provisioning.
- General host socket passthrough beyond the SSH-agent service explicitly named
  here.

## Impact

- Affected specs: `runtime-bootstrap`, `session-relay`
- Affected code:
  - `src/configuration/raw.rs`
  - `src/configuration/types.rs`
  - `src/configuration/loaders.rs`
  - `src/configuration/paths.rs`
  - `src/commands/check.rs`
  - `src/commands/shared.rs`
  - `src/mcp/server/service.rs`
  - `src/relay/lifecycle.rs`
  - `src/runtime/paths.rs`
  - `src/tmux/lifecycle.rs`
  - `src/acp/worker_driver/`
  - `src/pty/transport/`
  - new container runtime/engine module under `src/`
