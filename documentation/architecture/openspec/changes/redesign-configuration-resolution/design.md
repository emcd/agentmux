## Context

Two principles already exist in this project, and association resolution is the
outlier on both.

The precedence ladder CLI > environment > files > defaults is implemented and
tested for relay settings. Association resolution instead ranks a per-worktree
override file above the environment and terminates in Git-derived filesystem
guessing (Git common-dir parent basename for bundle, working-tree basename for
session).

Green MCP startup is stated in `runtime-bootstrap`: relay connectivity failures
must leave startup successful and surface at tool-invocation time. The same
requirement then contradicts itself with a scenario failing startup on an unknown
bundle.

The precedence ladder assumes its top tier carries a human's invocation-time
intent. For `agentmux host mcp` the command line is a version-controlled,
template-generated configuration file, so that assumption is false exactly where
it matters most: the most committed, least deployment-specific source occupies
the highest-precedence tier. Bring-up authoritatively knows the identity it is
starting and has no channel capable of outranking that template.

Several defects share this root. A CLI flag intended to redirect roots does
nothing in release builds. One override file is honored in release while its
sibling in the same directory is silently inert, because each has bespoke lookup
logic. Starter hydration scaffolds fresh configuration when an explicit root is
wrong, hiding the mistake.

## Goals / Non-Goals

**Goals:**

- One precedence ladder, applied consistently, with each tier ranked by how
  deployment-specific its source is.
- A channel through which bring-up can state identity authoritatively without
  requiring edits to generated client configuration.
- One effective-file resolver shared by every consumer, so override reachability
  cannot vary per file.
- Startup that fails only when the process cannot serve the MCP protocol.
- Removal of build-profile-dependent behavior from configuration resolution, and
  of Git metadata from association resolution. Build-profile gating of the state
  and inscriptions roots, and the Git-derived provenance feeding them, are
  intentionally retained here.

**Non-Goals:**

- Runtime instance selection, state and inscriptions root resolution, and their
  migration. State is relay-scoped rather than tree-scoped, and the relay socket,
  locks, ready sentinel, principal store, and peer credentials are relay-wide
  above the per-bundle layer. Changing where they live requires deliberate
  coexistence and cutover handling and belongs in its own change.
- Any identity mechanism beyond bundle and session. A general local agent
  identity document remains exploratory.
- Deep merging of configuration content.

## Decisions

### Configuration root resolution, with explicit tiers replacing

`--configuration-directory` > `AGENTMUX_CONFIGURATION_DIRECTORY` > opt-in
nearest-ancestor discovery > XDG/home default.

XDG sits in the default tier rather than the environment tier because it is a
location convention, not an expression of intent about this invocation.

Explicit tiers **replace** the root rather than prepending to a search list.
Alternative considered: a prependable search path across roots. Rejected because
an explicitly supplied test root would then fall through to a developer's real
configuration for any file it does not define, which is the same class of defect
this change exists to remove.

### Discovery is opt-in and default off

Discovery walks ancestors for a directory that *is* the configuration root,
making the marker self-describing. Alternative considered: retain the existing
marker, which parses `Cargo.toml` and matches our own package name. Rejected as a
coupling to build metadata that cannot describe a non-Agentmux repository
legitimately hosting Agentmux configuration.

Default off because discovery is a convenience for bare CLI invocations;
explicit paths already express intent, and silently preferring a repository-local
root over the user-level one is a hazard when the two describe different
deployments. Opt-in keeps the safe case the default while replacing an implicit
build-profile inference with an explicit, addressable choice.

### The overlay travels with the root

Each resolved root expands to `[root/overlay, root]` for file lookup, regardless
of how the root was selected. This is what allows overlay behavior without
modifying MCP client arguments, which is the originating requirement.

Alternative considered: anchor the overlay to the Git working-tree root
independently of the configuration root. Rejected because it requires Git
awareness, and because committing the configuration directory already makes the
overlay per-working-tree by construction.

> **Reversed before archive.** The second half of that rationale no longer
> holds: the configuration directory is not committed. See Reversal Recorded
> Before Archive at the end of this document. The first half — that anchoring to
> the working-tree root requires Git awareness — stands on its own and is still
> the operative reason.

Lookup selects the first existing regular file. A malformed overlay file is an
error, not a cue to fall through to the base: falling through would silently
apply configuration the operator believed they had overridden. Directories of
bundle definitions union by identifier with overlay entries shadowing base
entries, since whole-directory replacement would force the overlay to restate
every bundle. Relative paths inside an overlay file do not rebase under the
overlay.

Layering is whole-file replacement rather than deep merge. Deep merge makes "what
is in effect" unanswerable without simulating the merge and cannot express
removal. Note this is distinct from a single file's optional fields falling back
tier by tier, which remains how association precedence works within one selected
file.

Accepted limitation: the overlay can shadow a bundle but cannot remove one.
There is no tombstone, so neither "replace the bundle set entirely" nor "delete
this one bundle" is expressible; a base bundle stays in the effective union
until its own file is deleted. Union was still preferred over whole-directory
replacement, which would make forgetting to restate a bundle silently drop it
rather than silently keep it. Whether removal earns a mechanism is deferred
until an operator actually needs it.

### Bring-up stamps context onto spawned members

Configuration load stamps authoritative context onto each coder-backed member's
merged spawn environment, upsert-if-absent so an operator-declared value wins.
Spawning transports already apply that environment, so a launched agent
propagates it to its `agentmux host mcp` subprocess.

The stamp is a general context-propagation mechanism rather than a fixed pair of
variables, because the deferred state and runtime-instance work will need to
propagate a state root through the same channel.

### Association ends in a recorded condition, not a guess or a crash

Bundle: `--bundle` > injected environment > overlay > `--default-bundle` >
unresolved. Session: `--session-name` > injected environment > overlay >
working-directory match against declared member directories > unresolved.

`--default-bundle` exists so generated client configuration can seed a bundle
from the *default* tier instead of impersonating invocation intent. Alternative
considered: asking the template to stop emitting a bundle. Rejected because the
bundle is genuine information the template holds; the defect was its rank, not
its presence.

Git-derived auto-discovery is deleted rather than demoted. It is the mechanism
that produced the original misbinding, it guesses an answer that is plausible and
wrong, and the working-directory match that replaces it for session is
declarative: the bundle file already states where each member lives.

A tier applies only when every tier above it is *absent*, never when one of them
supplied a value that failed to resolve. A supplied identity carries intent, so
an unresolvable one is a fault retained with its own cause rather than a cue to
try the tier below. Otherwise a mistyped `--session-name` would descend into the
working-directory match and authenticate as whichever member owns the current
directory -- the same inference this ladder exists to remove, one tier down. This
applies to `--default-bundle` too: a name written into generated client
configuration is still a name someone wrote down.

The tier that supplied each identity is recorded alongside the identity and
carried into the startup inscription. The injected environment is the one tier
whose delivery depends on an agent harness passing it through to a grandchild
process, and a resolution that silently fell below it is otherwise
indistinguishable from one that never needed it.

No flag is added to request an unassociated server. Omitting a bundle yields one,
and the recorded reason distinguishes a deliberate relay-wide server from a
misconfigured one.

### Startup fails only when the protocol cannot be served

After `host mcp` is identifiable, the process retains an explicit
`Ready(context) | Unavailable(fault)` state and serves either way. Process-time
failure remains only for faults before that point, for async runtime, router,
stdio, or protocol serving failures, and for `--help`.

MCP negotiates tool inventory at `initialize`. A startup failure therefore erases
the advertised surface rather than degrading it, and agents subsequently call
tools their context says exist; some harnesses do not recover. Startup failure
also delays session initialization, and a fault delivered to an agent is far more
likely to be repaired than one written to a log the agent never reads.

This is not graceful degradation and does not conflict with the project's
fail-fast posture. Nothing is silently defaulted; the fault is retained, loud,
and structured, and it fires where the actor who can repair it will see it.
Degradation would be guessing a bundle when none resolves, which is precisely the
behavior being deleted.

Constraints: never proceed on partially parsed arguments or fall through from
malformed higher-level intent; keep `initialize`, tool listing, schemas, and
`help` green; validate each tool request before consulting the readiness guard so
a malformed call reports its own fault rather than the retained one; snapshot the
retained fault until restart. Two boundaries are decided rather than left to
implementation: the MCP list surface keeps its synthetic associated-home payload
as an explicit exception for an unreachable relay, rather than returning
`relay_unavailable`; and the broader relay rejection taxonomy, including
transport errors currently collapsed into generic I/O errors, is deferred as
out of scope here.

### Removing build-profile-dependent behavior, but only where it is safe

Behavior gated on `cfg!(debug_assertions)` conflates optimization level with
deployment mode, so a release binary run by a developer from a repository is
misclassified. For configuration-root resolution and the TUI session override the
concept is removed rather than re-gated: once the marker is "this tree contains
configuration", nothing developer-specific remains.

The state and inscriptions roots are deliberately left gated. Their
repository-local override is currently the only mechanism preventing a
source-tree relay and an installed relay from resolving the same relay socket,
locks, ready sentinel, principal store, and peer credentials, all of which are
relay-wide. Ungating them without runtime instances would silently collapse two
live deployments into one, stranding existing sessions on one relay while new
clients attach to another whose configuration may not define the same bundles.
Removing that gate is therefore part of the deferred work, not of this change.

The consequence is that this change makes the configuration and state roots
resolve by different rules for the duration. That asymmetry is temporary and
deliberate, and is preferable to coupling a corrective change to a migration.

It also constrains how far the Git removal can go here. The retained state
branches are only reachable when a repository root resolves, and that resolution
is itself Git-derived: a source-checkout probe and a Git common-dir lookup.
Deleting them as part of "removing Git" would leave the repository root
permanently unresolved, so the retained branches would never activate and
repository-local state would silently collapse onto the XDG default — the exact
coexistence failure this change defers. Association's use of Git is therefore
removed here; state provenance's use of Git is retained and removed by the
runtime-instance work that replaces it.

Retaining it also means reducing it to one answer. The probe and the common-dir
lookup were not two implementations of one rule; they were two rules, and they
disagree wherever it matters most. From a linked worktree the common-dir lookup
returns the owner root while the probe returns the worktree, so `host relay`
started in a worktree binds its socket where none of its own clients — CLI, TUI,
`host mcp`, all of which took the other path — would look. From a subdirectory
of a checkout the probe returns nothing at all while Git resolves the root, so
the same invocation silently changes deployment depending on which directory it
was issued from. Neither surface is wrong in isolation, which is why the
divergence survived: it is only visible by asking who else answers the question.

The two are therefore folded into one resolver, composing what each was right
about. Git supplies the candidate root, which buys worktree agreement and
ancestor search; the package-manifest marker then confirms it, which keeps the
repository-local branch confined to an actual Agentmux checkout rather than
whichever repository the process happens to stand in — a check the common-dir
path never had, and whose absence let a debug-build CLI adopt any unrelated
clone's `.auxiliary/` as its state root. Because the resolver is what the
retained provenance *is*, it is deleted by the same runtime-instance work that
deletes the branches it feeds, and by nothing sooner.

## Risks / Trade-offs

- **Deleting auto-discovery makes previously working invocations fail to
  associate.** → They now start green and report the reason on first tool call,
  which is the discoverable failure mode rather than a silent wrong binding.

- **Green startup could let an agent work for some time against a server that can
  never succeed.** → Every tool call fails immediately with the retained cause,
  and the cause names the actual defect. The alternative erases the tool surface
  entirely, which is strictly less recoverable.

- **Re-anchoring the overlay under the configuration root changes which file is
  found.** → Repository-local overlays now require either an explicit root or
  discovery enabled. Both are stated, and the previous behavior depended on Git
  awareness being removed here.

- ~~**Committing the configuration directory means working trees stop
  inheriting configuration changes without an update.** → Accepted
  deliberately; the overlay is the supported mechanism for per-tree
  divergence.~~ **Reversed before archive** — the configuration directory is not
  committed, so this trade-off never arises. See the reversal note below.

- **The renamed flag and the `--default-bundle` migration require an upstream
  template change.** → Coordinated with the template owner; alpha policy permits
  the rename without an alias.

- **Whole-file overlay replacement forces an override to restate a file's whole
  content.** → Accepted for legibility. Bundle directories union by identifier,
  which covers the case where restating everything would be most painful.

- ~~**Ordering constraint.** The repository's configuration directory must be
  committed before any invocation relying on a relative configuration path
  works.~~ **Reversed before archive.** No configuration directory is
  committed, so this ordering constraint does not exist. The hydration change
  it referred to still stands on its own: an explicitly supplied root that does
  not exist is a recorded fault rather than something scaffolded silently. See
  the reversal note below.

## Migration Plan

Sequenced so each step leaves the tree green and independently reviewable:
stamping substrate, then configuration root resolution and dev-mode removal,
then the overlay resolver, then the association ladder, then startup policy, then
the Git removal cascade that the preceding steps unlock, then committing the
repository's configuration directory.

Rollback is per-step; no data migration is involved because state layout is
explicitly out of scope.

## Open Questions

None outstanding. Both prior questions are resolved and specified:

- `--repository-root` loses its configuration-root role here and **retains** its
  state and inscriptions role until the deferred runtime-instance work replaces
  it. Retention is not a convenience: the repository-local state branches are
  only reachable when a repository root resolves, so removing the flag's
  remaining role would collapse repository-local state onto the XDG default.
- Discovery **always** reports the selected root, on a diagnostic channel that
  is never the MCP stdio stream, since writing to that stream would corrupt the
  protocol.

## Reversal Recorded Before Archive

Three passages above assumed the project would commit its own Agentmux
configuration directory, Git-ignoring an `overlay/` beneath it. That posture was
reversed during implementation and never carried out; the two VCS-posture
requirements are REMOVED by this change rather than modified.

Every file under a configuration root proved to be maintainer-specific:
`policies.toml` encodes one operator's lane topology, `users.toml` names a
person, `coders.toml` records locally installed coders and their prompt regexes,
and bundle members carry absolute worktree paths. The reasoning that led here
treated absence of absolute paths as portability; the test is whether a second
maintainer would want the file's contents, and for every file the answer is no.

Each affected passage is annotated in place, and in each case the reasoning that
did *not* depend on the committed directory is called out as still operative —
Git-awareness as the reason for rejecting a working-tree anchor, and the
hydration change as a fault rather than silent scaffolding. The successor work
is `agentmux:todos/general/35` and `agentmux:todos/general/36`; the layering
shape replacing `overlay/` is proposed as `layer-configuration-roots`.

Annotated rather than rewritten, so a later reader sees that the decision
changed rather than believing it was never made. The corresponding note in
`proposal.md` carries the same record for that document.
