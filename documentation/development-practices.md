# Development Practices

This guide distills project-specific development practices for agentmux.
It replaces the multi-file `.auxiliary/instructions/` references in `AGENTS.md`
with focused guidance for this Rust + TOML codebase.


## General Principles

### Robustness Principle

Be conservative in what you send; be liberal in what you accept. Validate
inputs early and produce well-formed outputs.

### Immutability First

Prefer immutable data structures when internal mutability is not required.
Use `&` references over `&mut` where possible. Default to `let` bindings
without `mut`.

### Dependency Injection

Functions should accept dependencies as parameters with sensible defaults
rather than reaching into global state. This improves testability and
makes coupling explicit.

### Error Chaining

Never swallow errors. Chain them with `.context()` or `.map_err()` to
preserve the causal chain. Use `anyhow::Context` for application errors
and `thiserror` for library/domain errors.

### Fail Fast

This project is **alpha software with live releases**. Do not preserve
backwards compatibility unless explicitly requested. Prefer raising errors
over "graceful degradation" with defaults; only use silent fallbacks when
explicitly requested.

### Function Length

Function bodies should not exceed thirty (30) lines. If you must scroll
to read a function, it is too long.


## Naming Conventions

### General Patterns

- Prefer single-word names: `name`, `count`, `timeout`, `callback`.
- Avoid repeating the struct or type name in field names:
  `Connection.timeout` not `Connection.connection_timeout`.
- Avoid truncations: prefer `configuration` over `config`,
  `arguments` over `args`.
- Use portmanteau words to avoid underscores: `datastore` not
  `data_store`.

### Rust Conventions

- **Types, traits, enums**: `PascalCase`
- **Functions, variables, modules**: `snake_case`
- **Constants, statics**: `SCREAMING_SNAKE_CASE`
- **Enum variants**: `PascalCase`

### Constants

- Use noun-first suffix patterns for semantic grouping:
  `TIMEOUT_DEFAULT`, `TIMEOUT_MAXIMUM`, `ENTRIES_MAXIMUM` — not
  `DEFAULT_TIMEOUT`, `MAX_TIMEOUT`, `MAX_ENTRIES`.
- Group related constants with common prefixes: `LOOK_LINES_DEFAULT`,
  `LOOK_LINES_MAX`.

### Scalar Count Fields

Name count fields noun-first with `_total` or `_count` suffix, distinct
from the vector they measure: `entries_total` (total available),
`returned_entries_count` (count in this response). This naming mirrors the
wire key shape.

### Function Naming

`<verb>_<noun>`: verb describes the action, noun describes the target.

Prefer Latin-derived verbs for domain operations: `validate_`, `resolve_`,
`authorize_`, `configure_`. Use Germanic-derived verbs for common data
operations: `load_`, `get_`, `make_`, `build_`, `find_`.

Avoid suffixes for implementation details (`_cached`, `_optimized`),
development status (`_experimental`), or debugging aids (`_verbose`).


## Code Comments

Comments should explain **why** code is written as it is — a hidden
constraint, a subtle invariant, a workaround for a specific bug, or
behavior that would surprise a reader. If removing the comment would not
confuse a future reader, do not write it.

**Never write comments that:**
- Reference external tracking by selector, label, or name: todo IDs,
  design labels (`D5`), OpenSpec change IDs, issue numbers, or notebook
  note references.
- Use implementation-status language: "MVP", "this slice", "Phase 2",
  "not part of the current contract", or any phrasing that implies
  temporary state without explaining the actual constraint.
- Explain what the code does — well-named identifiers already do that.
- Span multiple lines. One short line maximum.

A future reader will not have access to the external documents these
references point to. Every comment must be self-contained.


## Module Organization

- `src/lib.rs` or `src/main.rs` as the crate root.
- For modules under 1000 lines: `src/**/<module>.rs` (single file).
- For modules over 1000 lines: split into `src/**/<module>/*.rs` with
  `src/**/<module>/mod.rs` as a thin re-export hub.
- Re-export important items at appropriate levels using `pub use`.

Architecture and design rationale live in `src/**/README.md` files.
Each subsystem README documents design decisions, invariants, and
constraints — not in separate ADR documents.


## TOML Practices

- Use hyphens, not underscores, in TOML key names: `connections-maximum`
  not `max_connections`.
- Use noun-first suffix patterns for semantic grouping: `timeout-default`,
  `connections-maximum`.
- Use single quotes for string values unless escapes are needed.


## Code Formatting

- Maximum 79 columns per line.
- Single-statement loop/condition/exception bodies may be placed on the
  same line as the introducing statement when sufficiently short.
- `cargo fmt` is authoritative; `.rustfmt.toml` defines project overrides.


## Testing Practices

- Prefer tests under `tests/unit` and `tests/integration` over inline
  `#[cfg(test)]` modules in `src/**`.
- Prefer tests that exercise public interfaces; avoid source-inclusion
  patterns used only to reach private internals.
- Inline `#[cfg(test)]` is permitted only when ALL of the following hold:
  1. The tested item is crate-private **by design** and making it testable
     externally would require widening visibility or adding a
     `#[doc(hidden)] pub` escape hatch that would become unintended API
     surface.
  2. No existing public interface exercises the same code path.
  3. The inline test block contains at most **one** `#[test]` function.
- If a candidate inline test fails any of these conditions, move it to
  `tests/unit` and widen visibility or restructure as needed.


## Pre-Commit Validation

Run validation before committing to avoid hook failures:

```shell
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Hooks run these automatically, but running them manually first saves
turnaround time.
