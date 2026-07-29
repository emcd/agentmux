## Why

The state root is currently selected by how Agentmux was *built*.
`resolve_state_root` and `resolve_inscriptions_root` each carry a
`cfg!(debug_assertions)` branch redirecting to a repository-local location, fed
by a Git-derived repository provenance that returns nothing in release builds.
That gate is the only thing keeping a source-tree relay and an installed relay
off the same socket, locks, ready sentinel, principal store, and peer
credentials, all of which are relay-wide rather than per-bundle.

Build profile is a proxy for "which deployment is this", and a poor one. It
expresses exactly two deployments, names neither, cannot be exercised by the
binary that ships, and cannot describe two production deployments at all. The
source already documents the mechanism as provisional and names this work as its
replacement.

Two facts force the timing rather than merely recommending it.

The provenance walk resolves the Git *common directory* owner, which is what
makes every worktree of a project share one root. The planned move from worktrees
to clones-of-clones destroys that property: each clone resolves itself, binds a
private relay, and cross-clone coordination stops, with no configuration that
repairs it because the mechanism does not read configuration.

Separately, nothing propagates the state root to spawned children. A relay
started with an explicit `--state-directory` spawns a coder whose
`agentmux host mcp` descendant re-derives its own root and looks for a socket
that is not there. Only `AGENTMUX_BUNDLE` and `AGENTMUX_SESSION` are stamped, and
no agentmux environment variable for the state root exists at all.

## What Changes

- **BREAKING** Remove the `cfg!(debug_assertions)` branches from state and
  inscriptions root resolution. A debug build stops silently resolving different
  runtime paths than a release build from the same invocation.
- **BREAKING** Delete the Git provenance: `repository_checkout_root`,
  `git_common_directory`, `repository_root_from_git_common_directory`,
  `cargo_manifest_declares_agentmux`, and `run_git`. This is the last Git
  subprocess on the runtime and startup path; Agentmux stops requiring `git` on
  `PATH` to run. Building with the optional `pty` feature still clones Ghostty
  from its build script, so Git remains a build-time dependency there.
- **BREAKING** Remove `--repository-root`. Its configuration-root role was
  dropped by `layer-configuration-roots`; this removes the remainder, which is
  the whole of what it still does.
- State root resolution becomes `--state-directory` > `AGENTMUX_STATE_DIRECTORY`
  > XDG > home, identical in every build profile. One state root is one relay;
  isolating a deployment means naming its state root, not inferring one.
- Add `AGENTMUX_STATE_DIRECTORY` as the environment tier for the state root,
  symmetric with `AGENTMUX_CONFIGURATION_DIRECTORY`, injected by the relay at
  spawn so a coder-backed member — and the `agentmux host mcp` descendant that
  inherits its environment — stays on the relay that launched it.
- **BREAKING** That injection is authoritative, overwriting an operator-declared
  `AGENTMUX_STATE_DIRECTORY` at any level. It is the one exception to
  upsert-if-absent stamping: the variable names the relay a child belongs to, so
  a declared value breaks the rendezvous rather than expressing a preference.
- The state root is normalized to a non-empty absolute path before use, and an
  empty `--state-directory` is rejected. A relative root re-resolves under each
  child's working directory, which silently defeats propagation.
- Generated client configuration SHALL NOT emit `--state-directory`, so a
  committed template cannot outrank the injected value.

## Capabilities

### New Capabilities

None. This removes an inferred mechanism and makes an existing one explicit.

### Modified Capabilities

- `runtime-bootstrap`: state and inscriptions root resolution lose their
  build-profile branches and gain an environment tier; the repository-local
  override requirement is removed; bring-up context propagation extends to carry
  the state root.
- `cli-surface`: `--repository-root` is removed, retiring the "until the deferred
  runtime-instance work replaces it" clause `layer-configuration-roots` left in
  place as this change's entry point.

- `environment-variables`: the Environment Variable Precedence requirement gains
  the single exception to upsert-if-absent stamping. That spec states
  "operator-declared wins" for stamped context, and the authoritative state-root
  injection contradicts it, so the exception belongs where the rule lives rather
  than only in `runtime-bootstrap`.

Two further capabilities were checked and rejected as unmodified rather than
listed on the strength of touching the same subsystem:

- `relay-identity` is **not** modified. Its only state-root reference is overview
  prose locating the principal store at `<state_root>/identity/principals.json` —
  phrased relative to the root, so it composes unchanged. No requirement names a
  location.
- `acp-client` is **not** modified. It specifies no filesystem location at all.

## Impact

**Sequencing.** This must land before the worktree-to-clones-of-clones topology
move, which otherwise gives each clone a private relay with no configuration that
repairs it.

**Code.** `src/runtime/paths.rs` is the centre: three `cfg!(debug_assertions)`
sites, both root resolvers, and the entire Git provenance block.
`RuntimeRootOverrides` loses `repository_root`. `BringUpContext` is **not**
extended: its `VARIABLE_NAMES` is documented as the names this context carries,
is held equal to `environment_entries` by test, and its length is consumed as the
count of load-time stamped entries, so adding a spawn-time variable to it would
contradict all three. The state root is injected at spawn instead, and a separate
enumeration of inherited context variables is added for the harness that
sanitizes them. `src/commands/shared.rs` and `src/commands/host/relay.rs` stop
threading `repository_root`.

**Operators.** Only deployments relying on the repository-local debug branch are
affected, and they keep their existing state by naming it. Both roots must be
named, because they are siblings rather than nested:
`--state-directory <checkout>/.auxiliary/state/agentmux` together with
`--inscriptions-directory <checkout>/.auxiliary/inscriptions/agentmux`. Supplying
only the first preserves credentials and session state while silently relocating
new inscriptions beneath the state root, splitting log history. Nothing moves for
anyone resolving XDG. The transition is nonetheless a stop-the-relay operation —
see the cutover note in design.

**Testing.** Check D of `.auxiliary/scripts/verify-release-binary.sh` asserts
precisely the debug/release divergence this change deletes, and must be rewritten
rather than re-run.

## Decisions Taken

- **Isolation is declared, not derived.** A second deployment is created by
  naming a second state root. No identifier is inferred from configuration, build
  profile, or repository location. This follows AuxBE's design-review constraint
  that instance selection "must be explicit/stable, not inferred from bundle name
  or build mode" (`agents-common:reviews/3`), and their minimum position that
  explicit state-root isolation suffices.
- **No `instances/` layer and no instance identifier.** AuxBE's review offered a
  named-instance model with an `<xdg-state>/agentmux/instances/<instance>/`
  layout as one of two options. That layout is not adopted: `--state-directory`
  already means "put state here", and an identifier that also selects a state
  root would be a second mechanism answering one question — the defect this arc
  exists to remove. Recorded as a deliberate departure from a reviewed design
  rather than left as silence.
- **No detection of a superseded repository-local root.** Warning an operator
  that repository-local state exists while they now resolve XDG would require
  ancestor-walking from the working directory, which is the mechanism this change
  deletes. Reintroducing it as a diagnostic would defeat the change; the
  transition is handled by documentation instead.

## Out of Scope

- **Splitting inscriptions from the state root.** Tracked as `todos/runtime/25`;
  confirmed a separate future proposal by operator decision (2026-07-29). The two
  touch the same resolver but answer different questions — this change is about
  the rendezvous every principal must agree on, while that one is ergonomic.
- **Named runtime instances as a first-class concept.** Superseded by explicit
  state-root selection, per the decision above. If multi-deployment operation
  later outgrows naming state roots directly, an instance layer can be added with
  the use case that justifies it.
