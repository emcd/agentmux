# OpenSpec audit harnesses

Verification harnesses for `scripts/verify-openspec-deltas.py`, the checker
behind the `lint-openspec-deltas`, `lint-openspec-citations` and
`lint-openspec-archive` pre-commit hooks.

**These are not for authoring changes.** If you are writing a delta, the
interface you want is the hooks, or `scripts/verify-openspec-deltas.py
<change-id>` directly. Nothing here is needed for that.

They exist for the person editing the checker. It encodes a lot of judgment
about what evidence means — when a delta counts as synced, when a rename's two
halves cancel, when an archive is being filed rather than amended — and most of
those decisions have no instance in today's corpus. Without these, that judgment
can be broken silently by a change that leaves every hook green.

## The safety contract

Every destructive verb these harnesses use lives in `probe-worktree.sh`, and
every one refuses to run unless the working directory is inside a throwaway
worktree the library created under `.auxiliary/temporary/` (git-ignored, so it
never appears as untracked noise). None of them can reach your working tree,
your index, or the real planning home.

Confirm that rather than taking it on faith:

```sh
.auxiliary/scripts/openspec/check-probe-guard.sh
```

It exercises the refusals, checks that the worktree does not outlive the script
that made it, and statically verifies that no destructive verb sits in a
function that skips the guard. If you add a verb, add a guarded wrapper to
`probe-worktree.sh` — the static check fails otherwise, and that is deliberate.

The one exception is `check-mutations.py`, which is Python and writes a single
scratch file at `.auxiliary/mutant-verify-openspec-deltas.py`. It refuses to
start if that path already exists and removes it in a `finally`.

## The harnesses

| Script | Answers |
|---|---|
| `check-probe-guard.sh` | Does the safety contract above actually hold? |
| `check-archives.sh` | Would today's archive gate have passed each archive at the commit that filed it? |
| `check-probes.sh` | Do the checker's branches work, including the ones no real change reaches? |
| `check-archive-scope.sh` | Does the gate fire on filing an archive and stay quiet on amending one? |
| `check-mutations.py` | Do the checker's self-tests actually fail when its decisions break? |

### check-archives.sh

```sh
.auxiliary/scripts/openspec/check-archives.sh [count]      # default 25
.auxiliary/scripts/openspec/check-archives.sh <dir-name>   # one archive
VERBOSE=1 .auxiliary/scripts/openspec/check-archives.sh 200
```

The gate's verdict is only meaningful at the moment of archiving, since it
compares deltas against live specs that later changes go on editing. So this
replays it at each historical archive commit with today's checker copied in.

Also the tool for **"was requirement X ever synced?"** — name the archived
change that introduced it and read the verdict. That is how the three lost
requirements in the corpus were found.

A flag is not automatically a defect; read the errors before concluding. Some
archived changes deferred requirements to a successor on purpose.

### check-mutations.py

Deliberately coupled to exact source strings, and expected to need updating
whenever the checker changes. A `STALE` line means the mutated text moved and
that mutation needs rewriting — it is not noise to skip past, because a stale
mutation defends nothing.

Note that one mutation is caught by the live corpus rather than by a self-test:
the self-tests call the classifier directly and cannot observe the call site, so
dropping the argument there leaves them passing. Its fixture is whichever
document in the corpus names its own archive. If none does, that mutation goes
quiet and proves nothing.

## What is deliberately not here

Some harnesses from the same work stayed in `.auxiliary/scribbles/` because they
are one-offs whose value expired with the change that motivated them — a stack
shape check hardcoding one stack's commit subjects, and a reproduction of a
single review round whose cases now live in the checker's own self-tests.
Versioning those would ship rot. If you need that shape again, write a new one
against the stack you actually have.
