## Why

Configuration layering is currently fixed at two layers with a hardcoded shape:
a root, and an `overlay/` subdirectory beneath it. That shape was chosen when
configuration was expected to live inside the project being worked on, and it
does not survive contact with how Agentmux is actually deployed.

Every file under a configuration root is maintainer-specific. Policies encode
one operator's lane topology, `users.toml` names a person, and `coders.toml`
records which coders that developer has installed. None of it belongs in the
Agentmux repository, so configuration moves out of the repository entirely, into
a separately managed directory. Once it lives outside a project, the "overlay
beneath the root" arrangement has nothing to anchor to, and the layering an
operator actually wants — a shared base plus an R&D or production variant — is
not expressible as one nested pair.

The number two was never derived from anything. Making the layer list explicit
at the invocation site replaces a fixed arrangement with one the operator
declares, and states the layering where a reader can see it.

## What Changes

- **BREAKING** Accept `--configuration-directory` repeatably. Each occurrence
  appends a layer; the resulting list is searched in order, first match winning.
- **BREAKING** Remove the `overlay/` subdirectory convention. A layer is an
  ordinary configuration root, and layering is expressed only by the list.
- **BREAKING** `AGENTMUX_CONFIGURATION_DIRECTORY` accepts a `:`-separated list
  with the same ordering. Paths containing `:` are unrepresentable through the
  environment; the repeatable flag is the escape hatch and this limitation is
  documented rather than worked around.
- An explicitly supplied layer list is **closed**: it replaces the tier stack
  rather than extending it, so no root outside the list is consulted for any
  file. Closedness governs which roots are searched, not what absence means —
  each artifact keeps its own absence semantics, so optional files stay
  optional.
- Empty layer elements are rejected, from an empty flag value or from a
  leading, trailing, or doubled separator in the environment form. An empty
  element must never mean the working directory, which would silently admit
  configuration from wherever a process was started.
- Bundle definitions continue to union by identifier, generalized from two
  layers to N, nearest layer winning per identifier.
- The configuration-root watcher observes every supplied layer and reconciles
  against their effective union.
- **BREAKING** Remove `--discover-local-configuration`. See Open Decisions.

## Capabilities

### New Capabilities

None. This changes how an existing capability resolves, not what Agentmux can
do.

### Modified Capabilities

- `runtime-bootstrap`: configuration root resolution becomes an ordered layer
  list; the overlay subdirectory is removed from effective-file resolution,
  bundle-definition union, association-file lookup, and TUI sender
  configuration; the watcher observes N layers. The two VCS-posture requirements
  are removed by `redesign-configuration-resolution` rather than here, since
  their premise died with the decision to keep configuration out of the
  repository.
- `cli-surface`: `--configuration-directory` becomes repeatable, and
  `--discover-local-configuration` is removed.
- `bundle-lifecycle`: overlay-specific bundle resolution contracts.
- `ui-surface-configuration`: overlay-specific `ui.toml` resolution contracts.

Every delta is authored from live specification text. Drafting was deliberately
held until `redesign-configuration-resolution` archived, because that change
rewrote most of the same requirements and deltas written against its delta text
would have replaced a baseline that no longer existed by the time they applied.

Enumerating the live specs found the surface smaller than a raw reference count
suggested: nine requirements carry overlay contracts, not the ~59 references
spread across them. Each is reproduced in full as MODIFIED, and scenario counts
were compared against the live text mechanically, since a MODIFIED delta
replaces a whole requirement and a dropped scenario is invisible to `--strict`.

`AGENTMUX_CONFIGURATION_DIRECTORY` is specified by `runtime-bootstrap` rather
than `environment-variables`, which covers variables configured *for* spawned
children.

`mcp-tool-surface` is **not** modified. Credential administration
(`new peer --write-config`, `change psk --write-config`) writes session
pre-shared keys under the **state root**, not configuration, so layering does
not reach it and no requirement about it changes. See design.md for why the flag
name misleads.

## Impact

**Sequencing.** This change depends on `redesign-configuration-resolution`
having been archived first. That change's deltas rewrite most of the same
requirements, so authoring these deltas against today's live text would replace
the wrong baseline. Every MODIFIED delta here must be written from the
post-archive text.

**Code.** `configuration_root` appears 319 times across 37 files, and 77
function signatures take `configuration_root: &Path`. Those become a
`ConfigurationRoots` value. The change is wide but mechanical and fully
compiler-checked; the abstraction introduced by
`redesign-configuration-resolution` generalizes rather than being replaced.
`effective_configuration_path` goes from first-of-two to first-of-N,
`effective_bundle_definitions` unions N directories, and the watcher already
fingerprints by supplying layer, which is exactly what N-way reconciliation
needs.

**Operators.** Any invocation relying on an `overlay/` subdirectory must move
that directory into an explicit layer. There is no compatibility shim.

**Documentation.** With no configuration committed anywhere in the repository,
maintainer documentation becomes the only description of the layer layout, and
must carry worked examples.

## Decisions Taken

- **`--discover-local-configuration` is removed.** No consumer was identified,
  and the case it was built for — a configuration root inside the project being
  worked on — is the case this change removes. Reviving it later with a
  justifying use case costs less than carrying a second, inferential answer to
  the question an explicit layer list answers by naming its target.
- **Configuration sources are introspectable through `agentmux check
  configuration`.** With an arbitrary layer list an operator cannot otherwise
  tell which copy of an artifact won, and a shadowed file can be present,
  valid, and entirely inert. It is default output, suppressed by `-q`/`--quiet`,
  and emitted on standard output before validation runs. See design.md.

## Out of Scope

- **Tombstones.** N layers make "remove what a lower layer defines" more likely
  to be wanted than two layers did, but no use case exists today, and shadowing
  may already suffice. Tracked as `agentmux:ideas/general/4` rather than
  guessed at here.
