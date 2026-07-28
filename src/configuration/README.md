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
- `paths.rs`
  - Helpers for resolving configuration artifact paths.
- `raw.rs`
  - Raw TOML serde shapes and internal validated coder target descriptors.
- `fields.rs`
  - Shared field normalization, id/group validation, and best-effort path
    canonicalization helpers.
- `targets.rs`
  - Session-shape selection and coder target validation/resolution.
- `loaders.rs`
  - Public load APIs and per-file parsing/validation orchestration.

## Overlay Resolution

Every configuration file resolves through the overlay, so overriding one is the
same operation regardless of which file it is: a root expands to the lookup list
`[<root>/overlay, <root>]`, nearest wins. `paths.rs` maps each logical artifact
to that lookup, and it is the *only* place that mapping exists.

Replacement is whole-file, not key-merging. An overlay copy of a file supplies
that file entirely; it does not contribute keys to the base copy. A malformed
overlay file is a fault rather than a reason to fall through to the base, since
falling through would silently run the configuration the operator was replacing.

Bundle definitions are the one directory-shaped artifact, and they **union by
identifier**: an overlay-only bundle is enumerated, and an overlay definition
shadowing a base one of the same identifier is enumerated once, at its overlay
path. The accepted consequence is that an overlay can shadow a bundle but cannot
remove one — there is no tombstone. Replacing a base set wholesale means
shadowing each member individually.

Relative paths inside a configuration file resolve against the same base
regardless of which layer supplied the file, so moving a file into the overlay
does not rebase what it points at.

## Invariants

- `crate::configuration::*` is the public import surface for callers.
- Raw TOML structs stay private to the configuration module.
- Every configuration *read* resolves through a `paths.rs` accessor. Joining a
  file name onto a configuration root in a loader bypasses the overlay, which
  does not fail — it silently reads the base copy while the operator believes the
  overlay is in force. Starter hydration is the deliberate exception: it writes
  to the base root directly, since scaffolding an overlay copy would shadow the
  file it just created.
- Loader behavior should preserve existing config schema and validation errors
  unless a spec or task explicitly changes them.
- Bring-up context (`BringUpContext`) is stamped onto an agent-spawning member's
  merged environment after the operator-declared layers, upsert-if-absent.
  Carrying further context means adding a field there rather than teaching the
  loader about another variable; `BringUpContext::VARIABLE_NAMES` is the
  enumeration consumers read when they need the names without values.
