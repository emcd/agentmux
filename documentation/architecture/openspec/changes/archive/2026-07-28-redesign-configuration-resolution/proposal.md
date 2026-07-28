## Why

Configuration precedence should be CLI > environment > files > defaults, and the
project already implements that ladder correctly for relay settings. Association
resolution does not: it ranks a local override file above the environment, and
ends in Git-based filesystem guessing.

That ordering is not merely inconsistent, it is unreachable in practice. An MCP
server's "command line" is itself a version-controlled, template-generated
configuration file, so the tier reserved for a human's invocation-time intent is
fed by the least deployment-specific source available. A committed `--bundle`
flag outranks a deployment-local override, which inverts precedence relative to
how specific each source actually is. Bring-up knows authoritatively which bundle
and session it is starting and has no channel that can outrank the template.

The same subsystem also diverges from a second principle the project already
states: that an MCP process starts successfully and reports failures when tools
are invoked. `runtime-bootstrap` asserts this for relay connectivity and
contradicts it four lines later for bundle association. Failing startup erases
the advertised tool inventory rather than degrading it, so agents call tools
their context says exist and some harnesses never recover.

## What Changes

- **BREAKING** Rename `--config-directory` to `--configuration-directory`, with
  no compatibility alias.
- **BREAKING** Rename the `overrides/` directory to `overlay/`, and re-anchor it
  beneath the configuration root rather than the Git working-tree root.
- **BREAKING** Remove `mcp.toml`'s `config_root` field. A file inside the
  configuration root may not redirect the configuration root.
- **BREAKING** Delete Git-based association auto-discovery for both bundle and
  session. Session resolution ends in a working-directory match against declared
  member directories, which is declarative rather than inferred.
- **BREAKING** MCP startup no longer fails on an unresolvable bundle or sender.
- Add `--default-bundle`, separating "assert an identity" from "supply a
  default", so generated client configuration can seed a bundle without
  outranking bring-up.
- Add opt-in nearest-ancestor configuration discovery, default off.
- Resolve the configuration root as `--configuration-directory` >
  `AGENTMUX_CONFIGURATION_DIRECTORY` > discovery > XDG/home default, where
  explicit tiers replace rather than extend the root.
- Introduce a single effective-file resolver over `[root/overlay, root]` used by
  every relay, TUI, CLI, and preflight loader, replacing per-file override logic.
- Stamp authoritative bring-up context onto each coder-backed member's spawn
  environment, and consult it as an association tier.
- Remove build-profile gating from configuration-root resolution and from the
  TUI session override, so a configuration override no longer silently does
  nothing in release builds. Build-profile gating of the state and inscriptions
  roots is retained here and addressed by the deferred runtime-instance work,
  because the repository-local state override is currently the only thing
  keeping a source-tree relay and an installed relay from colliding.
- Restrict starter-configuration hydration to defaulted roots, so a wrong
  explicit path is reported rather than scaffolded over.

Runtime instance selection, state and inscriptions roots, and their migration
are deliberately out of scope; they are separately deployable, materially
riskier, and not required here.

## Capabilities

### New Capabilities

None. This change redistributes and corrects existing behavior.

### Modified Capabilities

- `runtime-bootstrap`: configuration root resolution, both association
  precedence requirements, the local override file's location and semantics,
  bring-up context injection, and generalizing relay-connectivity startup
  tolerance into a startup policy covering every non-protocol fault.
- `cli-surface`: `--configuration-directory` rename, `--default-bundle` and the
  discovery flag, removal of the configuration-root role of `--repository-root`,
  and deferred argument validation for `host mcp`.
- `mcp-tool-surface`: tool responses carry the retained startup fault as a
  structured, actionable cause.
- `ui-surface-configuration`: UI configuration becomes overlay-aware rather than
  configuration-root-only.
- `bundle-lifecycle`: the bundle watcher observes both physical layers and
  reconciles against the effective union, so shadowing and reveal are reloads
  rather than unloads.
- `environment-variables`: the environment tier's rank in association
  resolution, and the stamped bring-up context.

`tui-surface` is deliberately absent: it references `users.toml` abstractly, so
re-anchoring that file under the overlay is transparent to it. The TUI override's
location and build-profile reachability are governed by `runtime-bootstrap`.

## Impact

- `src/runtime/`: `paths.rs` (root resolution, dev-mode removal),
  `association.rs` (ladder, Git removal, `WorkspaceContext` collapse),
  `tui_session.rs` (override anchoring and build-profile gating),
  `starter.rs` (hydration policy).
- `src/configuration/`: loader stamping and the shared effective-file resolver.
- `src/commands/`: `host/mcp.rs` startup state machine, plus roughly ten call
  sites that thread a Git-derived workspace root purely to locate override files.
- `src/mcp/`: readiness guard and error surfacing for every tool.
- `src/relay/`: configuration watcher handling of overlay creation, deletion,
  reveal, and shadow transitions.
- Removes association's dependency on Git metadata. Git-derived provenance for
  the state and inscriptions roots is deliberately retained, because deleting it
  would leave the repository root unresolved and silently collapse
  repository-local runtime data onto the XDG default. Removing the remaining Git
  usage belongs with the deferred runtime-instance work.
- ~~Requires the repository's own Agentmux configuration directory to be
  committed, with `overlay/` ignored.~~ **Reversed before archive.** See the note
  below.
- Requires the upstream Copier template to emit `--default-bundle` instead of
  `--bundle`. Tracked as operator work outside this change.

## Reversal Recorded Before Archive

This proposal originally required the project to commit its own Agentmux
configuration directory, Git-ignoring an `overlay/` beneath it, and added a
`Override Directory VCS Posture` requirement saying so. That posture was
reversed during implementation and never carried out.

Every file under a configuration root proved to be maintainer-specific:
`policies.toml` encodes one operator's lane topology, `users.toml` names a
person, `coders.toml` records locally installed coders and their prompt regexes,
and bundle members carry absolute worktree paths. The reasoning that led here
was that these files contained no absolute paths and were therefore shareable;
absence of absolute paths is not portability. The test is whether a second
maintainer would want the file's contents, and for every file the answer is no.

Consequently the two VCS-posture requirements are REMOVED by this change rather
than modified, nothing replaces them, and configuration moves out of the
repository entirely. The successor work is
`agentmux:todos/general/35` (migration) and `agentmux:todos/general/36`
(maintainer guide). The layering shape that replaces `overlay/` is proposed
separately as `layer-configuration-roots`.

The record is annotated rather than rewritten so a later reader sees that the
decision changed, rather than believing it was never made.
