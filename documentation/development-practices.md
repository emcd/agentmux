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

### User-Facing Command Verbs

CLI subcommands and MCP tool names are a separate vocabulary from internal
function naming above, and follow the Germanic convention throughout:
`new`, `send`, `look`, `up`, `down`, `change`, `drop`. This holds regardless
of the Latin-preference default for internal domain-operation functions —
see the next section for how a command's verb and its backing
implementation's verb relate.

### A Command's Backing Implementation Echoes the Command's Verb

A function whose entire purpose is to directly implement one named
user-facing command (CLI subcommand or MCP tool) takes that command's verb,
unconditionally — not as an exception to some other default, but because a
command and its sole backing implementation are the same operation viewed
from two layers, and tracing from one to the other should never require a
vocabulary translation.

This is distinct from internal plumbing that no single command names —
cleanup helpers shared across multiple call paths, or invoked from more than
one place, with no one command whose verb they should echo. That plumbing
follows the ordinary naming conventions above (Latin-preferred for domain
operations, Germanic for common data operations, or an established local
convention such as `remove_<resource>` for resource cleanup where one
already exists) — it is a different case, not a default this rule carves an
exception out of.

Where a resource concept spans both a TOML configuration shape and its Rust
representation (a field name, a struct, the functions that act on it), the
same principle applies: keep the verb and noun identical across that
boundary. Divergent naming between what a TOML key says and what the Rust
interface calls the same concept is a recurring source of confusion when
tracing a value from configuration through to the code that consumes it.

An OpenSpec proposal introducing a new user-facing verb SHALL state it
explicitly and cite this convention, e.g. "the command verb is `drop`, per
the Germanic convention for operator-facing verbs." This puts the naming
decision in the document under review, rather than leaving it to surface
for the first time in implementation.


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

Current architecture lives in `src/**/README.md` files. Each subsystem README
documents how that subsystem is built now, together with its invariants and
constraints. A README stays accurate because anyone changing the code is
already reading it.

A closed decision, and the alternative it rejected, goes to
`documentation/decisions/` instead. The split is by tense rather than by
topic: a README says what *is*, and gets rewritten when the code moves; a
decision record says what was settled and why an attractive alternative was
not taken, and stays true afterwards.

So do not describe current architecture in a decision record, and do not leave
a rejected alternative's reasoning only in a README, where the next rewrite
drops it silently.


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

**Do not use plain `cargo test`.** This project's process-spawning and
global-state tests assume nextest's per-test process isolation. `cargo
test` shares one process per test binary, so tests that would pass in
isolation collide with each other's process-global state under it —
producing failures (and even hangs) that look exactly like a real
regression in whatever change is under review, but aren't. A red `cargo
test` on this repo is not evidence about the code; only a red `nextest`
run is. (Some of this collision is itself tracked as a defect —
`todos/backend/2` — but the underlying architectural reason does not
change the operational rule: always run tests through nextest.)

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

The generation-fence tests are **not** `#[ignore]`d and need no special
invocation. They once were, on the grounds that holding a bootstrap open
across a signal cost tens of seconds, and a pre-push hook ran them by
module selection. Both are gone: the whole `acp::generation_fence`
module now runs inside the default suite in a few seconds. If you find a
reference to a `cargo-nextest-generation-fence` hook or to running that
module with `--run-ignored`, it is stale -- no such hook is configured
and the selector would match nothing.

Do not reach for `--run-ignored ignored-only` across the suite. Every
remaining `#[ignore]` in this repository marks work that is blocked or
unimplemented rather than merely slow, so running them fails by design
rather than by accident. The Pty fence tests are a separate category
again -- they need Zig 0.15.x and the `pty` feature:

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

## Notebook Todo Status

For notebook todo notes, the title checkbox is the authoritative completion
state. Create todos without `#status-open` and do not add or maintain a status
tag when closing one. Use `nb.do` and `nb.tasks` to update and query todo state.

Do not rewrite existing todo notes merely to remove legacy status tags. They are
redundant but harmless; a destructive whole-note rewrite risks losing unrelated
content solely to remove duplicated state.

Status tags remain appropriate for non-todo notes, such as reviews and
coordination records, because those notes have no checkbox state.

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

**A correction is a first draft.** The rewrite that fixes a known error is the
least-audited sentence you will write, because you are in the posture of having
just verified something and the replacement inherits confidence it has not
earned. Both times this was caught here, the second error landed *inside the
sentence rewritten to fix the first* — once contradicting, two sentences later,
a mechanism the reviewer had supplied in the preceding message. Re-read a
correction against the source with the same suspicion you would give an original
claim, and expect no credit for the part you just got right.

When a mutation or an experiment does not produce the failure you predicted,
that gap is the finding: probe the branch for the state that decides it rather
than reasoning about why it might be fine. A green result you cannot explain is
never evidence for the explanation you happen to prefer. See also
[Absence Assertions Need a Positive Control](#absence-assertions-need-a-positive-control),
which is the same error wearing a test's clothes.

## Delegated Review Workflow

This project follows `.auxiliary/agents/procedures/reviews.md` (Copier-owned,
`agents-common`). The following tightens one path within it rather than
overriding it; capture local deviations here rather than editing the
Copier-owned file directly.

### Equivalent Rebase Assessment

After reviewer approval, an author rebasing onto an advanced
`<local-integration-base>` may submit a merge handoff when the rebased stack
is byte-equivalent to the approved artifact and validation passes on the
rebased exact hash. The handoff identifies the approved and rebased hashes,
provides patch-identity evidence, and explains the changed-file intersection
with intervening base changes.

The integrator assesses whether those intervening changes create a material
or subtle conflict. The integrator may accept the handoff without repeat
technical review when they do not, or require updated technical review when
they do. An author may not self-authorize this exception. Any content change
after approval, including a documentation correction, requires updated
technical review regardless of patch-identity evidence.

### Integration Batching

Default to one merge commit for a coherent implementation stack, such as a
completed OpenSpec implementation. Do not create a merge commit for every
independently reviewed implementation tranche.

Authors retain approved tranche commits on their lane branch and stack later
tranches on top. Each tranche receives technical review at its natural
checkpoint, but approval alone does not make it a merge handoff. The
integrator merges the accumulated approved stack at the planned integration
checkpoint.

Merge earlier only for an explicit integration checkpoint: a prerequisite
unblocks another lane, a security fix needs release, or the operator directs
separate integration. State that reason in the merge handoff.

## Pre-Commit Validation

Run validation before committing to avoid hook failures:

```shell
cargo fmt --check
cargo clippy -- -D warnings
cargo nextest run --locked --config-file .auxiliary/configuration/nextest.toml
```

Hooks run these automatically, but running them manually first saves
turnaround time.

### OpenSpec Delta Retention

`openspec validate --strict` checks that a delta is well formed. It never
compares a delta against the spec it modifies, so it cannot see the mistake
that actually costs content: a `## MODIFIED Requirements` delta replaces the
**entire** named requirement at sync time, and every scenario not carried
forward is deleted from the live spec.

`scripts/verify-openspec-deltas.py` audits that, and the
`lint-openspec-deltas` hook runs it for every change whose delta specs appear
in a commit. Its output prints even when the hook passes: dropping a scenario
is often exactly right, so retention is a report rather than a failure, and a
report nobody sees is the failure it exists to prevent. Read each `DROPPED`
line and confirm it names behavior the change retires.

Run the script by hand in the one case no commit can reach -- when the live
spec moves beneath an already-committed delta because another change archived
into the same requirement. The drop set changes there without any delta file
being touched, so the hook never fires:

```shell
scripts/verify-openspec-deltas.py <change-id>
```

### Durable Content From a Change

A change's `design.md` is change-scoped. At archive it moves into
`changes/archive/` with the rest of the change and is never synced anywhere,
so anything durable left in it goes dormant: the archive is not a spec, and a
live spec that cites one is a defect rather than a reference.

This is not a small residue. There are 84 archived `design.md` files holding
roughly 13,000 lines, within a few percent of the entire live spec corpus.

Write durable content to its home **during the change**, not at archive time:

| Content | Home |
|---|---|
| Required behavior | the change's spec deltas |
| Justification for one requirement | inline, in that requirement |
| A closed decision and the alternatives it rejected | `documentation/decisions/` |
| How a subsystem is built | the subsystem's `README.md` |
| How an algorithm works | a comment beside the code |
| Planning, sequencing, scratch reasoning | leave it in `design.md` to go dormant |

`documentation/decisions/README.md` defines the record format and the rule
that keeps such records from rotting.

Writing during the change rather than at archive is deliberate: the archive is
the moment the author is least motivated to review several hundred lines, and
the reasoning is freshest while the work is live. The drift this invites is
narrow, because a decision record states what was *rejected* rather than what
was built, and a rejected alternative does not go stale as the implementation
moves. The one real exposure is a decision reversed mid-change, which is what
the archive-time check exists to catch.

### Rust Module Line-Count Limit

A Rust module whose line count exceeds 999 is treated as a request to split.
A module of exactly 1000 lines already fails; the cap is a hard 1000-line
ceiling. The cap is enforced mechanically by the `linecheck` pre-commit
hook and the matching CI step in `.github/workflows/tester.yaml` so the
rule does not drift into a prose-only convention -- the same defect
`agentmux:procedures/2`'s proof-of-absence gate warned about. The
threshold is `warn: 800` (printed, non-blocking) and `error: 999` (blocks
the commit and the pull request).

There are no exceptions. The gate is unconditional: a pre-existing
over-limit module blocks this hook on every commit until it is split;
growth past an in-limit module's current count blocks the same way. There
is no inline ignore marker and no per-file allowlist in the configuration
-- an entry in `.auxiliary/configuration/linecheck.yaml` for a specific
path would be the same graveyard the proof-of-absence gate records, and
the no-exception requirement forbids both.

`linecheck` reads its rule definition from
`.auxiliary/configuration/linecheck.yaml`. The config covers two patterns
(`src/**/*.rs` and `tests/**/*.rs`) at the same thresholds, and an
`exclude:` list that opts markdown, generated build artifacts, and vendored
sources out of `linecheck`'s built-in default fallback. The pre-commit hook
runs on every commit (`always_run: true`, `pass_filenames: false`) so the
rule cannot be bypassed by a commit whose staged files happen to not match
the gate's filter.

The hook fails the chain before any expensive Rust hook (`cargo fmt`,
`cargo clippy`, `cargo nextest`) runs, so the cost of a 1001-line commit
attempt is a sub-second warning and exit, not a full toolchain setup.

There is no window after archiving: the script resolves `changes/<change-id>`
and an archived change no longer lives there.
