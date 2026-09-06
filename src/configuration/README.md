# Configuration Layout

This directory loads and validates operator configuration artifacts, then
normalizes them into runtime structures used by CLI, MCP, relay, and TUI code.

## Modules

- `mod.rs`
  - Public re-exports and shared private constants.
- `types.rs`
  - Public configuration structs and enums used by downstream runtime code.
- `errors.rs`
  - `ConfigurationError` and error formatting/source support.
- `roots.rs`
  - `ConfigurationRoots`: the ordered layer list, its construction from the flag
    and environment forms, and the rejection of empty elements.
- `paths.rs`
  - Helpers for resolving configuration artifact paths across the layers.
- `raw.rs`
  - Raw TOML serde shapes and internal validated coder target descriptors.
- `fields.rs`
  - Shared field normalization, id/group validation, and best-effort path
    canonicalization helpers.
- `targets.rs`
  - Session-shape selection and coder target validation/resolution.
- `loaders.rs`
  - Public load APIs and per-file parsing/validation orchestration.
- `bindings.rs`
  - Validation of the `[bindings]` group in `ui.toml`: resolving action and
    context names against the TUI vocabulary, parsing chords, and refusing a
    binding a context cannot perform.
  - The binding sets this build ships, and the reader that turns one into rows.
    Their text lives in `data/bindings/` and is embedded with `include_str!`.

## Layer Resolution

Every configuration file resolves through the same layer list, so overriding one
is the same operation regardless of which file it is. `ConfigurationRoots`
(`roots.rs`) holds an ordered, non-empty list of roots; a lookup walks it front
to back and stops at the first layer holding the file. `paths.rs` maps each
logical artifact to that lookup, and it is the *only* place that mapping exists.

**The first layer wins.** These are search-path semantics (`PATH`,
`LD_LIBRARY_PATH`, `-I`), not cascade semantics: nothing is merged or applied on
top of anything, so an override goes at the *front* of the list. That surprises
anyone picturing layers stacked bottom-up, which is why the flag's help text and
the environment variable's documentation both state which end wins rather than
leaving it to be inferred.

A supplied list is **closed**: no root outside it is consulted for any file, so
naming a root means that root and a typo is an error rather than a silent
demotion to a different deployment. Closedness governs *which roots are
searched*, not what absence means — an artifact absent from every layer keeps
whatever absence semantics it already had, so optional files (`mcp.toml`,
`users.toml`, `ui.toml`) stay optional.

An empty layer element is rejected, from an empty flag value or from a leading,
trailing, or doubled separator in the environment form. The search-path
convention this otherwise follows reads an empty element as the working
directory; configuration selects policies and identities, so reading a layer
from wherever a process was started is a privilege question rather than a
convenience.

Replacement is whole-file, not key-merging. A copy in an earlier layer supplies
that file entirely; it does not contribute keys to a copy in a later one. A
malformed file is a fault rather than a reason to fall through, since falling
through would silently run the configuration the operator was replacing.

**A lookup can fail, and only one filesystem answer counts as absence.** The
layer walk stops at the first layer that *supplies* the file, and it also stops
at the first layer that cannot be asked. Nothing existing at the candidate path
means that layer does not supply it and the walk continues; a permission error,
a non-directory component along the relative path, or a path occupied by
something other than a regular file all fault instead. Reading any of those as
absence produces the same observable result as falling through for a malformed
file — the shadowed layer's value quietly takes effect — except with nothing
logged, since no file was ever opened to fail on. Bundle enumeration applies the
same rule at each of the three points it touches the filesystem: opening a
`bundles/` directory, taking each directory entry, and typing each `.toml` entry.
Only an absent `bundles/` directory is absence, so a layer never contributes a
silently short set that reads as the definitions it holds.

Bundle definitions are the one directory-shaped artifact, and they **union by
identifier**: a bundle only one layer defines is enumerated, and a definition
shadowing one of the same identifier in a later layer is enumerated once, at the
path that supplied it. The accepted consequence is that a layer can shadow a
bundle but cannot remove one — there is no tombstone. Replacing a set wholesale
means shadowing each member individually.

Relative paths inside a configuration file resolve against the same base
regardless of which layer supplied the file, so moving a file between layers
does not rebase what it points at.

## Binding Configuration

The `[bindings]` group in `ui.toml` is validated here rather than by its
consumer, which is why this module reads the TUI's binding vocabulary
(`crate::tui`). A configuration naming a behavior, context, or chord that does
not exist is a fault in the file, and an operator should hear about it from the
loader that read the file rather than from whatever later failed to find the
name — including `agentmux check configuration`, which already loads `ui.toml`
and so gets the diagnosis for free.

That direction is one-way: `src/tui` imports nothing from here, and the two are
wired together a layer up, in `runtime/tui_session.rs` and `commands/check.rs`.
Returning validated structures rather than raw ones is also what keeps the
raw-structs-private invariant below intact; handing a consumer the parsed TOML
to interpret would have broken it.

Which actions a context may bind is derived from that context's compiled rows
rather than from a list kept here. The table declares a behavior only where it
has an effect, so its rows already are the answer, and a second list could
disagree with them.

Chord values are read as `toml::Value` and interpreted by hand. The natural
serde spelling is an untagged enum, but an untagged enum reports only that a
value matched no variant, naming neither the chord nor the key at fault, which
is not a diagnosis an operator can act on.

A group is accepted or refused whole. The first fault rejects it rather than
applying the rows that happened to parse, because a configuration half in force
is one an operator cannot reason about: which half survived would depend on
where in the file the mistake sat.

### Why bindings merge while files replace

Layer resolution above selects one `ui.toml` and reads it; a file in a nearer
layer replaces the one it shadows rather than merging with it. Binding rows
inside the selected file then merge *over* the compiled defaults, row by row.
The two rules look inconsistent and are not, because they answer different
questions.

A file is a unit an operator wrote and can read end to end. Merging two of them
would produce a configuration existing in no file, whose effective content an
operator could only derive by simulating the merge — and layer resolution exists
precisely so they can tell which copy is in force.

The compiled defaults are not a file, are not authored by the operator, and
change between releases. Replacing them wholesale would mean every operator who
wanted one different chord had to restate the entire table, and would freeze
that release's table into their configuration — so a later release's new or
corrected bindings could never reach them. Merging row by row is what keeps a
configuration a list of departures rather than a fork.

That is also why nothing here scaffolds a defaults file to disk. Writing one
would convert the defaults from something a build supplies into something an
operator owns, with the same freezing effect and no way back.

### Chord collisions

Two written chords can denote the same keystroke — `ctrl+j` and `control+J`, a
symbolic chord that resolves onto a literal one, or a bare character and its
shifted form, since a bare character denotes that character both bare and
carrying `Shift`. A group naming both is refused rather than resolved by file
order, which would make precedence an accident of how TOML keys happened to
sort.

The check reads every keystroke a chord's resolved shape denotes, under both
resolutions of the symbolic modifier, so a file is accepted or refused the same
way wherever it is read rather than colliding only on the platform that resolves
the symbol onto a chord the file also names.

### Shipped binding sets

A set an operator adopts by name is a configuration file embedded in the binary,
read by the same parser that reads their own — not rows constructed in code.
Each set is therefore a conformance test of the grammar, and a worked example
that cannot drift from what the parser accepts.

Its rows are parsed when a configuration names the set, and its names resolve
alongside every other name in the group, so a set naming a behavior this build
does not have is reported where the operator's own faults are.

A set that fails to parse is reported as `MalformedEmbeddedArtifact`, never as a
fault in `ui.toml`. The text is fixed at compile time and the parser is the one
the repository's checks exercise, so the fault is in our artifact and no edit to
their file changes it; that variant carries no path, so no consumer downstream
can name their file from it either.

Which capability class a set applies to is carried by its rows, in the format an
operator writes: a set for terminals that report modified keys distinctly states
the `enhanced` column and no other, so it contributes nothing where the probe
reports the other class. Saying it that way rather than with a separate class
field is what makes the restriction structural — a set cannot be brought into
force where the keystrokes it moves behavior onto cannot arrive, because there
is no arrangement of a configuration that would do so.

## Invariants

- `crate::configuration::*` is the public import surface for callers.
- Raw TOML structs stay private to the configuration module.
- Every configuration *read* resolves through a `paths.rs` accessor. Joining a
  file name onto a single layer in a loader bypasses the list, which does not
  fail — it silently reads one layer's copy while the operator believes another
  is in force. Starter hydration is the deliberate exception: it runs only for a
  defaulted list, which is a single layer, and writes to that layer directly.
- Loader behavior should preserve existing config schema and validation errors
  unless a spec or task explicitly changes them.
- Bring-up context (`BringUpContext`) is stamped onto an agent-spawning member's
  merged environment after the operator-declared layers, upsert-if-absent.
  Carrying further context means adding a field there rather than teaching the
  loader about another variable; `BringUpContext::VARIABLE_NAMES` is the
  enumeration consumers read when they need the names without values.
