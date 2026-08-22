## Context

Two derived lists govern agentmux context variables. `BringUpContext` enumerates
what configuration load stamps onto a member's spawn environment;
`INHERITED_CONTEXT_VARIABLE_NAMES` enumerates what a consumer sanitizing an
inherited environment must clear. Both are built from names defined together in
`configuration/types.rs`, and the comment there states the invariant: a name held
elsewhere is a name those lists silently omit.

The configuration-directory name is held elsewhere — a private constant in
`runtime/paths.rs`, reachable only by the resolver that reads it. Neither list
can see it, so members are never handed the relay's configuration root and test
harnesses never clear it. Both reported symptoms are the same omission observed
from different directions.

Configuration resolution differs structurally from state resolution in a way that
matters here. State is a single root, normalized to an absolute path after
resolution precisely so it can be propagated. Configuration is an ordered layer
list whose explicit and environment tiers pass their declared paths through
unnormalized, resolving against the process working directory at lookup time —
behavior the maintainer guide documents as intended. The environment
representation of that list is separator-delimited, and the tier that reads it
ranks below the command-line flag.

## Goals / Non-Goals

**Goals:**

- A spawned member reads the declarations of the relay that spawned it, without
  the operator naming a configuration directory in generated client config.
- The inherited-context sanitization set covers every stamped name, so a
  developer's exported environment cannot leak into a test run.
- The structural cause is removed, so the next context variable added cannot
  repeat the omission.

**Non-Goals:**

- Making configuration-directory authoritative over operator declarations.
- Redesigning the separator-delimited environment grammar to admit paths
  containing the separator.
- Changing the coder client configuration templates, which live outside this
  repository.
- Altering configuration layer precedence or the closed-list semantics of the
  explicit and environment tiers.

## Decisions

**Stamp as ordinary context, not as a second authoritative exception.** The
state-root exception is justified in the live spec by what the variable
addresses: it names the relay a member is a child of, and a wrong value breaks
the rendezvous. Configuration-directory does not meet that bar. The socket,
session and peer pre-shared keys, and the principal store all resolve beneath the
state root, so a member with a divergent configuration root still finds and
authenticates to its relay; bundle and session context already outrank
configuration in association resolution, so it is not misidentified either. What
diverges is the set of declarations it reads.

Alternative considered: authoritative spawn-time overwrite, mirroring the state
root. Rejected on two grounds. It would assert a guarantee the resolution model
cannot honor, because the environment tier ranks below the command-line flag and
a declared `--configuration-directory` outranks any injected value. And the
override case it would suppress has a better answer already expressed in the
spec: a test wanting different configuration should launch a relay with its own
roots, not re-point a child, exactly as cross-relay reach is expressed by
configured peers rather than by re-attaching children.

The distinction is worth stating in the spec rather than leaving implicit,
because "the child must read what the relay reads" is a plausible-sounding
rendezvous argument that does not survive checking where the socket and
credentials actually live.

**Stamp at configuration load, not at spawn.** The spec's stated reason for the
state root being spawn-time is that the value is unknown at load. That reason
does not transfer: the layer list is what load read the configuration from. Load
time keeps this on the existing `BringUpContext` path, which the code already
describes as agnostic to which variables it carries, instead of adding a second
spawn-time injector whose only precedent exists for the opposite semantics.

The existing context mechanism carries the new variable almost unchanged, with
one exception. Its entry enumeration produces every pair eagerly and the stamping
loop then discards those whose name is already declared, which is harmless for
values that are already borrowed strings but not for one that must be
constructed and can fail to be representable. That eagerness has to be resolved
where the layer list is concerned; the rest of the mechanism is untouched.

**Absolutize the layer list at root resolution.** Without this the stamping
mechanism is correct and the outcome is still wrong: a member that declares its
own working directory resolves a stamped relative layer against its own
directory. The precedent and the reasoning already exist for the state root,
where normalization is described as a precondition for propagation rather than
tidiness, and where the observation that members routinely declare their own
working directory is already recorded.

Normalization is lexical absolutization against the relay's working directory,
matching how the state root is normalized. Canonicalization is rejected: it
resolves symlinks, and symlinked configuration paths are ordinary in this
project, so canonicalizing would rewrite an operator's declared layer into a path
they never named.

Doing this at resolution rather than at stamp time keeps one resolved value for
both the relay's own lookups and the stamp. Absolutizing only for the stamp would
mean the relay and its members resolve from values that differ before either is
used, which is the split this change exists to close.

**Absolutization does not reach socket addressing, and the separation it relies
on is already established.** Absolutizing a root for propagation has collided
with `sockaddr_un.sun_path` in this project before: `runtime/sockets.rs` records
that a relative path used to be the escape hatch producing a short string to
bind against, that state-root normalization removed it, and that the short
string is now reconstructed by addressing through a parent directory descriptor
on Linux, with Darwin keeping the pre-existing reach.

That module also states the principle this design follows: the two requirements
are separable, because the root must be absolute so a spawned child resolves the
same directory whatever its working directory, while the string handed to `bind`
or `connect` is a different string that nothing requires to be the same one.

The collision does not recur here, because configuration layers never become
socket addresses. Both sockets this project binds — the relay socket and each
bundle's tmux socket — resolve beneath the state root, which is already absolute
and already carries the reconstruction. Configuration layers are lookup roots for
declaration files and are never bound. The one place configuration load bears on
`sun_path` is that it supplies each member's working directory, so the tmux
client runs from the bundle runtime directory and the address stays short; that
value comes from the session's declared directory field inside a configuration
file, not from the layer path, so absolutizing the layer list does not move it.

The stamped environment value grows, since absolute layers are longer than
relative ones, but an environment value is bounded by the combined argument and
environment limit rather than by `sun_path`, and a joined layer list is orders of
magnitude below it.

**Reject an unrepresentable layer list only where a stamp is actually
required.** A layer path containing the separator cannot be encoded in the single
delimited value. Splitting it would fabricate layers the operator never declared;
omitting the stamp would silently return the member to the default tier, which is
the defect.

Failing at root resolution was rejected because it would refuse a configuration
that never needs the representation. Two cases never need it: a coder-less member
is not stamped at all, and a coder-backed member declaring its own
`AGENTMUX_CONFIGURATION_DIRECTORY` keeps that value under the upsert-if-absent
contract, so the relay's list is never serialized for it. Conditioning on the
bundle merely containing a coder-backed member would still reject the second
case. The condition that matches the contract is whether a stamp is about to be
written.

This has an implementation consequence worth stating, because the obvious
construction violates it: joining the layer list eagerly to build the context
would evaluate the representation for every member, including those that never
receive it, and would fail a configuration the requirement says must load. The
serialization, and the validation that guards it, belong at the point where the
stamp is written.

The error names the offending layer. The repeatable command-line flag remains the
way to express such a path, and remains sufficient for any deployment that does
not need the value stamped.

## Risks / Trade-offs

**A deployment relying on lookup-time re-resolution of a relative configuration
layer changes behavior** → This is the breaking element and it is documented
today, so it needs a release note rather than a compatibility shim. A shim would
have to keep the relative value for the relay's own lookups while stamping an
absolute one, reintroducing the split. Operators naming an absolute layer, which
the default tier always produces, are unaffected.

**Members that currently scaffold a fresh default root stop doing so** → This is
the intended correction, but it is observable as "a member that used to start now
reports missing configuration." That is the honest outcome: the previous behavior
produced an empty deployment that appeared to work. It gets its own scenario so
it is not discovered as a side effect.

**The stamp remains outranked wherever generated client configuration still emits
`--configuration-directory`** → The prohibition added here is normative for this
repository, but the templates are distributed with the shared agent tooling. The
requirement makes the template change obligatory rather than optional; until it
lands, template-generated deployments continue to resolve from the flag. This is
worth tracking explicitly, because the operator-facing motivation for the change
is precisely that committed configuration should work without per-worktree
overrides, and that outcome is not reached inside this repository alone.

**A layer path containing the separator becomes a hard failure where it
previously worked** → Only for relays spawning coder-backed members, and only
where such a path is in the effective list. The error names the layer and the
repeatable flag remains available. The alternative is a member silently reading
fabricated or default roots.

## Migration Plan

The behavioral change reaches operators through configuration, not through a data
migration. A relative `--configuration-directory` or
`AGENTMUX_CONFIGURATION_DIRECTORY` becomes absolute against the relay's working
directory at startup; an operator who intended lookup-time re-resolution names
the intended absolute path instead. The maintainer configuration guide documents
the current asymmetry as intended and is corrected in the same change, so the
guide never describes a behavior the code no longer has.

Rollback is reverting the change: nothing is persisted in a new format and no
state is rewritten.

## Open Questions

None blocking implementation. The sequencing of the client-configuration template
change relative to this one is a coordination question outside this repository,
recorded under Risks rather than resolved here.
