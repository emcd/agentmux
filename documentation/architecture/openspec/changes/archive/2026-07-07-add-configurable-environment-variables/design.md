## Context

Environment support today is ACP-only and lives in the transport-specific
`[coders.<id>.acp].environment` subtable, applied at `acp/client.rs`
`spawn_command`. Tmux and Pty coders cannot declare environment; bundles and
sessions have no environment surface. This change generalizes environment to a
transport-agnostic, three-level layered surface.

The coder-target-descriptor requirement in `session-relay` is concurrently
being MODIFIED by the unarchived `add-pty-transport` change (adding
`[coders.pty]`). To stay archive-order-independent, this change introduces
environment as **orthogonal ADDED requirements** rather than modifying that
requirement.

## Goals / Non-Goals

- Goals: declare environment at coder, bundle, and session levels; a single
  well-defined merge; apply on all spawning transports; retire the ACP-only
  environment field.
- Non-Goals: templating/interpolation of values; environment for non-coder
  (`ui`/`pubsub`) sessions; secret management or value encryption; per-command
  (initial vs resume) environment differences.

## Decisions

- **Declaration type**: reuse `NameValueEntry { name, value }` — already used
  for ACP `headers` and the old ACP environment, and already validated by
  `validate_name_value_entries`.
- **Precedence: session > bundle > coder**, resolved per-variable
  (most-specific-wins), with non-colliding names unioned. Rationale: a coder in
  `coders.toml` is a reusable template shared across many bundles (the most
  global/base layer); a bundle narrows it (e.g. a prompt-injection-risk bundle
  overriding `OPENCODE_ENABLE_EXA` off); a session is the finest instance.
- **Merge once at load**: the loader computes the merged environment and stores
  it on the resolved `BundleMember` (new `environment: Vec<NameValueEntry>`).
  The merge lands in `loaders.rs`, where the `RawBundleFile`, `RawSession`, and
  resolved coder are all in scope; `targets.rs::build_session_target` sees only
  `&RawSession` and would require threading bundle-level environment through its
  signature, so it is not the merge site. Transports read `member.environment`
  at spawn; no transport re-implements the merge. The ACP path stops reading
  `AcpTargetConfiguration.environment` (which is removed) and reads the merged
  member environment.
- **Lift the ACP field (BREAKING)**: move `environment` off `RawAcpTarget` /
  `AcpTargetConfiguration` up to `RawCoder` / the merged member environment.
  `headers` remain ACP-specific (HTTP-only concept). Per alpha-defaults the old
  key is simply dropped — `deny_unknown_fields` already rejects it; no bespoke
  rejection logic, error code, or "reject old key" test/scenario is added.
- **Non-spawning targets**: a declared environment that resolves onto a target
  with no local child process (ACP `http` channel, `ui`/`pubsub` markers) is
  **inert**, not a load error. Alternative considered: fail-fast reject a
  coder-level environment on an `http`-only coder. Rejected because
  bundle/session environment legitimately fans out across a bundle that may mix
  spawning and non-spawning members, so rejection would create an awkward
  asymmetry; a benign no-op matches the existing ACP-`http` behavior and keeps
  the merge composable.

## Risks / Trade-offs

- Tmux environment goes through tmux's own model (`new-session -e KEY=VALUE`),
  not a plain `Command::env`; the wiring must land on the pane/session-creation
  path, not the auxiliary tmux control commands. Mitigation: apply `-e` flags
  at the coder session-creation call site only.
- Breaking config move for existing ACP environment users. Mitigation:
  explicit BREAKING flag in proposal, tasks, and commit message; alpha allows
  it.
- Inert-on-non-spawning softly deviates from the alpha-defaults fail-fast
  preference ("prefer raising errors over graceful degradation"). Accepted
  because bundle/session environment legitimately fans out across a bundle that
  may mix spawning and non-spawning members, so a benign no-op is required for
  composability; a hard reject would break fan-out. The deviation is scoped to
  this inertness only.

## Migration Plan

- Existing `[coders.<id>.acp].environment` entries move up one level to
  `[coders.<id>].environment`. No runtime shim; the raw loader rejects the old
  location via `deny_unknown_fields`.

## Open Questions

- None blocking. Value interpolation and secret handling are explicitly out of
  scope for this change.
