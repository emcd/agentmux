## Context

`redesign-configuration-resolution` established a single overlay-aware lookup:
each configuration root expands to `[<root>/overlay, <root>]`, first existing
regular file winning, with whole-file replacement rather than key merging. That
design solved the problem it was aimed at — every configuration file overriding
by the same rule instead of each growing a bespoke mechanism — and its
abstraction is what makes this change cheap. What it fixed at two was the layer
*count*, and the subdirectory name anchoring the second layer.

Two facts have since invalidated the shape rather than the abstraction.

Everything under a configuration root is maintainer-specific. Policies encode
one operator's lane topology; `users.toml` names a person; `coders.toml` records
locally installed coders and their prompt regexes. Absence of absolute paths in
a file does not make it portable, which is what made this easy to misjudge: the
test is whether a second maintainer would want the file's contents, and for
every file the answer is no. Configuration therefore moves out of the Agentmux
repository into a separately managed directory.

Once configuration lives outside a project, `overlay/` has nothing to anchor to,
and the layering operators actually want — a shared base plus an R&D or
production variant — is a list, not a nested pair. The arity was never derived
from a requirement; it was the smallest number that solved the original case.

## Goals / Non-Goals

**Goals:**

- Replace the fixed two-layer arrangement with an operator-declared ordered list
  of configuration roots.
- Keep resolution semantics unchanged: whole-file replacement, first match wins,
  bundle definitions union by identifier.
- State layering where a reader of the invocation can see it, rather than in a
  directory convention they must already know about.
- Preserve the property that an explicitly supplied root never silently degrades
  to a different one.

**Non-Goals:**

- Key-level merging across layers. Whole-file replacement stays, for the reason
  it was chosen: a partially-merged file is a configuration no operator wrote.
- Tombstones. N layers make removal more likely to be wanted than two did, but
  no use case exists today and shadowing may already suffice.
- Any change to state or inscriptions root resolution. Those are Proposal B's.
- Migrating the operator's own configuration out of the repository. That is
  operational work tracked separately.

## Decisions

### Precedence is first-wins

The list is searched front to back, first existing file winning, so the first
layer is the highest-precedence one and the last is the base.

The deciding argument is that these are search-path semantics, not cascade
semantics. Nothing is merged or applied on top of anything: a lookup finds a
file and stops. Every Unix mechanism with that behavior — `PATH`,
`LD_LIBRARY_PATH`, `PYTHONPATH`, `-I` include directories — orders first-wins,
and prepending is the established gesture for adding an override.

The alternative, last-wins, reads naturally if one pictures layers stacked on
top of each other, and matches the general CLI convention that later flags
override earlier ones. It was rejected because that mental model implies a
cascade this design deliberately does not implement. Pairing cascade ordering
with search semantics would leave operators reasoning about a merge that never
happens.

Environment consistency reinforces it. `AGENTMUX_CONFIGURATION_DIRECTORY` must
be `:`-separated, since environment variables cannot repeat, and every
`:`-separated search variable in Unix is first-wins. Last-wins on the flag would
put the two surfaces in opposite orders — a worse inconsistency than the
representability limit below.

The trade-off is honest and must be documented at the flag itself: the override
goes *first*, which surprises anyone thinking in stacked layers.

### The supplied list is closed

When a layer list is supplied, it replaces the tier stack: no root outside the
list is consulted for any file.

This preserves the property `redesign-configuration-resolution` established —
naming a root means that root, so a typo becomes an error rather than a silent
demotion to a different deployment. An explicit list is compatible with that
rule in a way an implicit search order would not be: the operator declared every
member, so nothing is inferred. The distinction is between an explicit list and
implicit fallthrough, not between one root and several.

Closedness is about *which roots are searched*, not about what absence means.
Conflating the two would break optional artifacts: `mcp.toml`, `users.toml`, and
`ui.toml` are legitimately absent in ordinary deployments, and a rule making
absence-from-all-layers a fault would turn each of them into a startup failure.
Each artifact keeps the absence semantics it has today.

### Empty layer elements are rejected

An empty element — from a flag with an empty value, or a leading, trailing, or
doubled separator in the environment form — is a validation error.

The `PATH` convention this design otherwise follows treats an empty element as
the working directory. That behavior is a known source of security bugs there,
and it would be worse here: configuration selects policies and identities, so
silently reading a layer from wherever a process happened to be started is a
privilege question, not a convenience. Following `PATH` on ordering while
departing from it here is deliberate, and the departure is stated in the
specification rather than left to implementation.

### Configuration has essentially one writer, and it needs no new rule

An earlier draft of this design required writes to target the first layer, on
the reasoning that a write to a shadowed layer is inert. That rule was removed
because the writes it named do not write configuration.

`new peer --write-config` and `change psk --write-config` resolve through
`resolve_config_credential_path`, which takes the **state root**: they stage
session pre-shared keys under
`<state-root>/bundles/<bundle>/sessions/<session>/identity.psk`, and the
destination is rejected for any principal that is not a session. The flag name
describes the operator's intent — write it down rather than return it — not the
tree it lands in. Specifying those writes as configuration writes would have
invited an implementer to move session credentials into a configuration root:
precisely the tree that is shared, layered, and potentially committed. That is a
security regression, not a naming quibble.

What remains is starter hydration, which is already restricted to a defaulted
list. A defaulted list is a single layer, so there is no ambiguity for a rule to
resolve. The decision is recorded here because its absence is otherwise
conspicuous: a reader comparing this to other layering systems will look for a
write-target rule and should find out why there is none.

### `overlay/` is removed rather than retained as sugar

A layer is an ordinary configuration root. Keeping `overlay/` as an implicit
extra layer beneath each supplied root would reintroduce the fixed arity inside
each element of a variable-length list, and leave two mechanisms answering one
question. Operators with an existing overlay name it as a layer instead.

## Risks / Trade-offs

- **A wide mechanical refactor.** 77 signatures take `configuration_root:
  &Path`. → The change is compiler-checked end to end, and the type is
  introduced once rather than threaded ad hoc; no signature can be missed
  silently.
- **First-wins surprises operators who think in stacked layers.** → Documented
  at the flag, in the environment variable's description, and in the maintainer
  guide's worked examples, each stating which end wins rather than implying it.
- **The environment variable cannot express paths containing `:`.** → Accepted,
  matching every other Unix search variable, with the repeatable flag as the
  escape hatch and the limit stated in the specification rather than discovered.
- **Removing `--discover-local-configuration` deletes a capability with no
  identified consumer but a plausible future one.** → Alpha defaults favor
  deleting over carrying speculative surface, and an explicit layer covers the
  same deployments by naming the target rather than inferring it. Reintroducing
  inference later is cheaper than maintaining two answers now.
- **More layers make the absence of tombstones more visible.** → Out of scope by
  decision, tracked separately, and recorded here so a future reader sees it was
  weighed rather than missed.

## Migration Plan

This change follows `redesign-configuration-resolution`, which rewrote most of
the same requirements. Its deltas apply at archive, so deltas here had to be
authored afterwards or they would have replaced a baseline that no longer
existed by the time they applied. That ordering has been observed: every
MODIFIED requirement here is reproduced from the live specifications as they
stand after that archive.

Operators move any `overlay/` directory to a sibling and name it as a layer
ahead of the base. There is no compatibility shim, per alpha defaults: an
`overlay/` subdirectory simply stops being consulted, and a deployment relying
on one resolves the base files it was overriding. Because that failure is
silent — the base configuration is valid and loads cleanly — the migration step
is called out in the maintainer guide rather than left to the changelog.

## Open Questions

- Does `--discover-local-configuration` have a consumer worth keeping it for?
  Recommended for removal; the operator has identified no specific use case, and
  its original one is being removed.
- Should the pre-flight command render the resolved list with the supplying
  layer for each effective file? It would make a shadowed file obvious, which is
  the failure mode layering introduces, but it enlarges output that is already
  the widest surface in `cli-surface`.
