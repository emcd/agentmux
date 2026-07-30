## Context

`redesign-configuration-resolution` deleted "dev mode" as a concept and removed
four of its five `cfg!(debug_assertions)` sites. Three survive deliberately:
`repository_checkout_root` and the repository-local branches of
`resolve_state_root` and `resolve_inscriptions_root`. Each carries a source
comment stating why it stayed and naming this work as its replacement.

They stayed because they were load-bearing. `RelayRuntimePaths` places
`relay_socket`, `relay_lock_file`, `relay_spawn_lock_file`, and
`relay_ready_sentinel` at the state root — *above* `bundles/<name>/` — and the
principal store and peer credentials sit there too. Nothing below the state root
isolates two deployments, so removing the gate without a replacement collapses a
source-tree relay and an installed relay onto one socket and one credential
domain. Both are live on the maintainer's machine today.

`layer-configuration-roots` then removed the flag's other half:
`--repository-root` no longer influences configuration-root resolution, and the
live `cli-surface` spec says it "SHALL retain its existing role in state and
inscriptions root resolution until the deferred runtime-instance work replaces
it." That sentence was written as this change's entry point.

## Goals / Non-Goals

**Goals:**

- Resolve the state root by one rule in every build profile.
- Make deployment isolation declared rather than inferred.
- Keep a spawned child on the relay that spawned it.
- Delete the last Git subprocess on the runtime and startup path, and with it the
  `git`-on-`PATH` requirement for running Agentmux. Building with the optional
  `pty` feature still clones Ghostty from its build script, so this is not the
  end of Git in the project — only in the binary's execution.
- Give an operator relying on repository-local state a path that does not move
  their credentials.

**Non-Goals:**

- Splitting inscriptions from the state root. Separate proposal; see
  `todos/runtime/25`.
- A named-instance model. See the decision below — explicit state-root selection
  replaces it rather than deferring it.
- Multi-tenancy. The state root remains `0700`; this separates one operator's
  deployments, not operators from each other.
- Running one relay across several state roots. One state root is one relay,
  which is what makes the socket rendezvous unambiguous.

## Decisions

### Isolation is declared by naming a state root

A second deployment exists because an operator named a second state root. Nothing
derives an identifier from configuration, build profile, or repository location.

This is the narrow reading of AuxBE's design review, which required that
selection "must be explicit/stable, not inferred from bundle name or build mode"
and set the floor at "retain explicit state-root isolation for R&D"
(`agents-common:reviews/3`). Inference is what the current mechanism does, and
every form of it shares the same defect: the operator cannot see, from the
invocation, which deployment they are talking to.

The alternative considered and rejected was deriving an identifier from the
configuration layer list. It is worth recording why, because it is superficially
attractive — it makes coexistence correct with no configuration at all.

It fails on the gesture the maintainer guide already recommends. Adding an R&D
override layer to an existing list is documented as *prepending a layer*. If the
list determines the state root, that gesture silently moves the operator to a
different relay: new socket, unrecognized credentials, running agents invisible,
and no error anywhere. A mechanism whose failure mode is "your override worked,
and also everything vanished" is worse than the one being replaced.

It fails a second time at cross-relay peering, where a peer's `address` is a Unix
domain socket path. A derived identifier makes that path unknowable before either
relay has run, so a configuration that must be written in advance cannot be.
Running several R&D relays to exercise inter-relay communication is established
practice, so this is not a hypothetical.

### No `instances/` layer

AuxBE's review offered a named-instance model with an
`<xdg-state>/agentmux/instances/<instance>/` layout. That layout is not adopted.

`--state-directory` already means "put state here." An instance identifier that
also selects a state root is a second mechanism answering one question, which is
the defect this arc exists to remove — `layer-configuration-roots` rejected
retaining `overlay/` as sugar on exactly that ground. One state root is one
relay, and the operator names it.

This is a departure from a reviewed design and is recorded as a decision rather
than passed over. What the instance model offered beyond explicit naming was
zero-configuration coexistence, and the analysis above is why that turned out to
be a liability rather than a feature.

### The state root gains an environment tier, mirroring configuration

Resolution becomes `--state-directory` > `AGENTMUX_STATE_DIRECTORY` > XDG > home.

The environment tier is not decoration; it is how propagation works. AuxBE's
review identified the requirement and named the variable: "If a relay starts with
an explicit state root/instance, its spawned coder and MCP child must inherit
that same state selection (for example `AGENTMUX_STATE_DIRECTORY`)." Adding it as
a resolution tier means the child needs no bespoke path — it resolves roots the
way every other invocation does, and the stamped value simply outranks the
default.

Symmetry with `AGENTMUX_CONFIGURATION_DIRECTORY` is the secondary argument. A
state variable that behaved differently from the configuration variable would be
a second thing to learn for no gain.

### The state root is injected at spawn, authoritatively; bundle and session stay load-time

Bundle and session context continue to be stamped at configuration load,
upsert-if-absent. The state root is injected by the relay at spawn, overwriting
any declared value.

Two properties force the split rather than taste. The value is not known at
configuration load — it belongs to the relay performing the spawn, not to the
configuration being loaded — so load-time injection would have to re-derive it,
which is exactly the re-derivation propagation exists to prevent. And
upsert-if-absent cannot express the contract: a declared or blank
`AGENTMUX_STATE_DIRECTORY` would suppress the stamp and point the child at a
different relay. That is not an operator overriding a preference; it is a broken
rendezvous, with the relay waiting for a client that never arrives.

The asymmetry is defensible because the two kinds of variable differ in kind.
Bundle and session name an identity a member may legitimately assert.
`AGENTMUX_STATE_DIRECTORY` names the relay the member is a child of, and a child
addressing a relay that did not spawn it has no meaning. Cross-relay
communication is expressed by configured peers, not by re-pointing a child.

### The state root is normalized to an absolute path before anything uses it

Resolution normalizes the state root to a non-empty absolute path, resolving a
relative value against the resolving process's working directory.

This is a correctness precondition for propagation, not tidiness. A relative
root re-resolves against each spawned process's working directory, so a relay
started with `--state-directory ./state` would stamp a value that means something
different to every child that runs from a different directory — and members
routinely do, since each declares its own directory. The failure is silent: the
child resolves a plausible path, finds no socket, and reports the relay
unavailable.

Empty is rejected on the flag rather than normalized. The environment tier treats
blank as absent, so accepting an empty flag would let one spelling of "nothing"
mean two different things depending on which surface carried it.

### Socket paths are bound through a directory, not by their full length

Normalizing to absolute paths removes an escape hatch that was load-bearing:
a relative state root produced a short string to bind against, which is how deep
directory hierarchies stayed under the `sockaddr_un` limit.

The budget is tight enough to matter. `sun_path` is 108 bytes on Linux, 107
usable. The deepest path this project constructs is
`<state_root>/bundles/<bundle>/tmux.sock`, which measures **96 bytes** for a
repository-local state root in a worktree checkout — 11 bytes of headroom. A
longer checkout path or a bundle name a few characters longer overflows it, and
the deployments most likely to name an explicit state root are exactly the deep
ones.

The resolution is that the two requirements are separable. The state root must be
absolute so a child resolves the same directory regardless of its working
directory. The string passed to `bind` or `connect` is a different string, and
nothing requires it to be the same one.

For sockets Agentmux binds itself, the parent directory is opened and the socket
addressed through that descriptor — `/proc/self/fd/<n>/relay.sock`, bounded at
roughly 27 bytes however deep the real directory is. The directory is already
`0700` and the descriptor is the process's own, so this adds no security surface.
`/proc` is Linux-only; elsewhere the same effect needs a working-directory
change, which is process-global and therefore worth avoiding where the
descriptor form is available.

The tmux socket needs its own answer, and it is the one that matters most since
it is the longest path. Agentmux does not bind it — tmux does, from the `-S`
argument it is given. The candidate is to set the spawned tmux process's working
directory to the bundle runtime directory and pass a relative `-S tmux.sock`,
which is available because Agentmux controls that spawn. That must be checked
against tmux's own use of its server working directory for default pane
directories before being adopted, so it is carried as an implementation task
rather than settled here.

A symlink into a short directory was considered and rejected. It does not solve
`connect`: the kernel resolves symlinks after the path is passed, so a client
still passes the long path unless it already knows the short one — at which point
the length problem has become a naming problem, which is the trap this proposal
removed elsewhere. `$XDG_RUNTIME_DIR` would be a safer home than `/tmp`, being
per-user, `0700`, and the basedir spec's designated location for sockets, but the
naming problem is unchanged.

Independently of technique, a path-length failure must surface as a structured
error naming the limit and the offending path. Today it reaches an operator as a
raw bind errno, which does not suggest the cause and is why the limit has bitten
this project before.

### The state root is propagated, not the socket path

The stamp carries the state root rather than the relay socket.

A child needs more than reachability. Session credentials resolve to
`<state_root>/bundles/<bundle>/sessions/<session>/identity.psk`, and MCP already
derives the socket from the state root via `RelayRuntimePaths::resolve`. One
variable therefore covers both; stamping the socket alone would fix reachability
and leave credential resolution still re-deriving from a different root — an
authenticated-as-nobody failure that looks like a credential problem rather than
a path problem.

### Generated client configuration must not emit `--state-directory`

Coder templates emit an `agentmux host mcp` command line. That command line is a
committed, template-generated file, so a flag in it is committed content wearing
CLI intent — the root cause `redesign-configuration-resolution` was written to
fix, where a VCS-committed `--bundle` outranked a deployment-local override.

A `--state-directory` in a template would reproduce it exactly, and worse: it
would outrank the `AGENTMUX_STATE_DIRECTORY` stamp and silently defeat
propagation, putting the child on a different relay than the one that spawned it.
AuxBE flagged this shape in review — "a committed state flag would recreate the
same 'template content wears CLI intent' problem." The prohibition is stated in
the specification rather than left as template convention, because the template
is not the only thing that could carry such a flag.

### The Git provenance is deleted with the branches, in one commit

`repository_checkout_root` and the two build-profile branches are removed
together.

Its own doc comment states the hazard: deleting the resolver without deleting the
branches leaves the repository root permanently unresolved, and the branches then
silently collapse repository-local state onto the XDG default — a data-location
change disguised as a cleanup. Sequencing them into one commit makes the hazard
unrepresentable rather than merely documented.

### The transition is documented, not detected

An operator whose relay currently runs on repository-local state will, after this
change, resolve XDG from the same invocation. Their running relay stays where it
is while new clients go elsewhere: split brain.

The tempting mitigation is to detect it — notice repository-local state while
resolving XDG, and warn. That is rejected because detecting it requires
ancestor-walking from the working directory to find the checkout, which is the
mechanism being deleted. A diagnostic that resurrects the deleted mechanism
defeats the change, and a partial version of it would resurrect the parts hardest
to reason about.

What remains is a cutover note. It is short because nothing moves, but it is not
one flag: the two repository-local roots are **siblings**, not nested.
`debug_repository_state_root` is `<checkout>/.auxiliary/state/agentmux` while
`debug_repository_inscriptions_root` is
`<checkout>/.auxiliary/inscriptions/agentmux`. Naming only the state root
preserves credentials and session state but silently relocates new inscriptions
to `<checkout>/.auxiliary/state/agentmux/inscriptions`, splitting an operator's
log history across two locations with nothing to indicate it happened.

The cutover therefore names both roots, or documents the history split
explicitly as a deliberate choice. Preserving both is the default advice, since
`relay.log`'s location is a convention the whole team reads against.

This satisfies AuxBE's signoff condition requiring a stopped-system contract when
repository-local state is removed, at the weight the actual change warrants:
stop the relay, name both roots or accept the split, restart. There is no
credential migration, because no credential moves.

## Risks / Trade-offs

- **A developer who relied on automatic repository-local state must now name
  it.** → The central trade of the change: one flag, or one environment variable
  in a shell profile, in exchange for a mechanism that is invisible, untestable
  in release, and about to break under clones-of-clones.
- **Two relays started with the same state root collide.** → Intended, and
  better than the alternative. The collision is loud and immediate — the spawn
  lock and socket are already contended — whereas silent separation is the
  failure this change exists to remove.
- **Split brain during the transition.** → Real, and mitigated by documentation
  rather than detection, for the reason above. Bounded by being a one-time
  operation affecting only deployments using the debug branch.
- **`--repository-root` disappears from a live CLI surface.** → Alpha defaults;
  no compatibility alias. Its configuration role is already gone, so what remains
  is a flag whose entire effect is the mechanism being deleted. Existing
  unknown-flag validation reports it without bespoke handling.
- **Two injection points now exist** — load-time for bundle and session,
  spawn-time for the state root. → A cost accepted for the reasons above, and
  bounded by being the only exception: the rule is "load-time upsert-if-absent
  unless the value belongs to the spawning relay", and exactly one variable
  qualifies today.

## Open Questions

None outstanding.
