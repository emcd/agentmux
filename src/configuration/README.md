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
