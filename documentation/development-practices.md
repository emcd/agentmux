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
- Use implementation-status language: "this slice", "Phase 2",
  "not part of the current contract", or any phrasing that implies
  temporary state without explaining the actual constraint.
- Explain what the code does — well-named identifiers already do that.

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

The canonical test runner is [cargo-nextest](https://nexte.st/). It is
installed as a standalone binary (not a Cargo dev-dependency):

```shell
cargo install cargo-nextest --locked
```

Run the suite locally with:

```shell
cargo nextest run --locked --config-file .auxiliary/configuration/nextest.toml
```

Per-repository configuration lives at
`.auxiliary/configuration/nextest.toml` (currently a slow-test warning
tripwire at 10s; do not raise the global threshold without a per-test
override for any test that legitimately exceeds it). The config file is
not at nextest's default autoload path (`.config/nextest.toml`), so the
`--config-file` flag is mandatory on every invocation -- forget it and
the slow-timeout warning silently won't trip.

Pre-commit hooks and CI also use `cargo nextest run --locked
--config-file ...`; if you only have `cargo test` installed, hooks
will fail.

Some tests are `#[ignore]`d because they are slow by nature rather than
broken -- holding a generation's bootstrap open across a signal takes
tens of seconds, and that is too much to pay on every commit. Those run
at pre-push via the `cargo-nextest-generation-fence` hook, selected by
module so a slow test added beside them is picked up automatically. Run
them by hand with:

```shell
cargo nextest run --locked --config-file .auxiliary/configuration/nextest.toml \
  --run-ignored ignored-only -E 'test(/^acp::generation_fence::/)'
```

Do not widen that to `--run-ignored ignored-only` across the whole
suite: most `#[ignore]`s in this repository mark work that is blocked or
unimplemented, not work that is slow, and running those would fail every
push. The Pty fence tests are a third category again -- they need Zig
0.15.x and the `pty` feature:

```shell
cargo nextest run --features pty --run-ignored all -E 'test(/^pty_transport::/)'
```

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

### Absence Assertions Need a Positive Control

An assertion that something does **not** happen is satisfied by a mechanism
that never runs. Pair it, in the same fixture, with a case where the thing
**does** happen — otherwise the test passes identically against working code
and against a feature that has been dead since the day it was written.

This is not hypothetical. `relay_send_async_emits_no_receipt_for_a_delivered_outcome`
asserted that a delivered outcome produces no terminal-outcome receipt. It
passed throughout a period when the relay emitted **no receipts at all**,
because every one was refused before it could be written — a live defect the
test was shaped to miss. The positive control that found it required a receipt
to arrive.

The same shape accounts for three further findings in `agentmux:issues/relay/68`,
where a batch barrier, a suppression rule, and a destructive fence step were each
"covered" by a test that its own mechanism's deletion could not fail.

When an absence assertion cannot be paired — the positive case is genuinely
unreachable — say so in the test's doc comment rather than letting the green
result imply coverage, and record what would make it reachable.

### Timing-Sensitive and Flaky Tests

Tests that wait on an external signal (a file-watcher event, a process
respawn, a scheduled callback) are exposed to real OS timing variance, not
just logic bugs. A handful of recurring failure shapes are worth designing
against up front:

- **Budget, don't sleep.** Wait for the expected condition against a
  deadline — event- or notification-based waits are often preferable to
  polling, since they avoid unnecessary observer load — rather than
  sleeping a fixed duration and hoping the work finished. On timeout,
  panic with the accumulated evidence (captured logs, last-seen state)
  rather than a bare "timed out" — the next person debugging a CI-only
  failure has no other way to see what actually happened.
- **Budgets tuned on one platform are not proven on another.** OS
  schedulers and filesystem-notification backends vary in latency and
  variance far more across platforms (and across loaded vs. idle
  machines) than most logic does. A budget that comfortably passes on
  Linux CI is not evidence it is generous enough for macOS, Windows, or a
  contended local machine — validate timing-sensitive tests in CI on
  every supported platform before trusting a fixed budget, and prefer an
  intentionally generous budget over a tight one when the cost of waiting
  a few extra seconds on failure is cheap. Note the asymmetry: asserting
  that at least a duration elapsed is safe, since another machine can only
  be slower, while asserting that elapsed time stayed *under* a ceiling is
  the fragile direction. Where a property genuinely needs an upper bound to
  be meaningful, prefer driving the code on a supplied clock and asserting
  the step count, which is exact and costs no wall time, over widening the
  ceiling until it no longer discriminates the thing it was written for.
- **Watch for self-triggered feedback loops.** A process that reads or
  writes the same files it is watching can retrigger its own watcher
  (e.g., filesystem "access" events fired by the watcher's own reconcile
  read). Filter or debounce events the system itself causes, not only
  events from external actors, or a test can pass by accident on a
  coincidental extra trigger and fail once that coincidence goes away.
- **Assert on identifying state, not on aggregate counts, in shared
  fixtures.** When a test fixture has multiple actors that can produce
  indistinguishable log lines or events (two workers emitting the same
  event name, for instance), an assertion on a global count is fragile —
  any actor's unrelated activity can shift the count. Assert on the
  specific actor or target under test instead.
- **Diagnose from evidence, don't guess at a fix.** Attempt targeted
  reproduction in an environment equivalent to where the failure occurred
  (e.g., stress-running the specific test under load). If it doesn't
  reproduce — a platform-specific or CI-load-dependent race may be
  impossible to trigger on a developer's machine — preserve the CI
  failure evidence and either add diagnostics/instrumentation to capture
  more on the next occurrence, or validate a concrete causal hypothesis
  against that evidence. A fix can be justified without local
  reproduction when the evidence proves the cause, but a speculative
  timing change applied on a hunch cannot: it can leave the actual race
  intact while appearing to close the issue, and a second, deeper
  investigation is sometimes needed when the first fix doesn't hold up
  under recurrence.
- **Quarantine is a last resort, not a resolution.** `#[ignore]`-ing a
  flaky test to unblock a merge should be paired with a tracked
  follow-up to de-flake and re-enable it. An indefinitely ignored test is
  a silent coverage gap.

## Claims and Evidence

### A Mechanism Existing Is Not a Mechanism Governing

The most common way a confident claim in this repository turns out false is
this shape: read code, confirm a mechanism exists and does what its name says,
then assert something about a **consequence** further down the path without
tracing the path. The mechanism is real; it simply does not decide the thing
being claimed. Nothing about the reading feels like guessing, which is why it
keeps happening.

Four instances, all recorded in the delivery-arc task history:

- The execution watchdog arms off `inflight_members`. True — and used to assert
  it "cannot see a `Pending` member", which is false: a held `Pending` member
  coexists with in-flight authorized ones, and the fail-stop branch resolves it.
- ACP's `fenced` flag is distinct from shutdown and correct for what it governs.
  True — and used to assert ACP "already has the right shape", which is false:
  `fence_generation` also clears the shutdown sender, so the delivery task reads
  a fence as a shutdown and reports it that way.
- Pty's `fence_generation` sets the same flag Tmux's does. True — and used to
  assert Pty had the same defect, which is false: Pty's drain spells no outcome
  at all, dropping its senders so the guard's evidence order answers.
- The relay reconciles an outcome against recorded evidence. True — and used to
  assert it would reconcile a shutdown-triggered resolution to
  `dropped_on_shutdown`, which is false: for an unbound member the producer's
  spelling stands untouched.

The discipline is to name the two things separately before writing the claim.
*This mechanism does X* is what you verified. *Therefore the caller reports Y*
is a second claim about a path you have not read yet, and it needs its own read
— of the consumer, not the producer. Where the consequence is the point of the
sentence, trace to the site that actually emits it and cite that site.

When a mutation or an experiment does not produce the failure you predicted,
that gap is the finding: probe the branch for the state that decides it rather
than reasoning about why it might be fine. A green result you cannot explain is
never evidence for the explanation you happen to prefer. See also
[Absence Assertions Need a Positive Control](#absence-assertions-need-a-positive-control),
which is the same error wearing a test's clothes.

## Pre-Commit Validation

Run validation before committing to avoid hook failures:

```shell
cargo fmt --check
cargo clippy -- -D warnings
cargo nextest run --locked --config-file .auxiliary/configuration/nextest.toml
```

Hooks run these automatically, but running them manually first saves
turnaround time.
