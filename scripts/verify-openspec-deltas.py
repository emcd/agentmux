#!/usr/bin/env python3
"""Audit an OpenSpec change's deltas against the live specs.

`openspec validate --strict` checks that a delta is well formed: every
requirement has a scenario, every requirement statement says SHALL or MUST. It
does not compare a delta against the specification it modifies, so a whole class
of authoring mistakes passes strict validation silently.

The load-bearing one is scenario retention. A `## MODIFIED Requirements` delta
replaces the **entire** requirement, not just the lines that changed, so any
scenario the author does not carry forward is deleted from the live spec at sync
time. When a requirement is substantially rewritten it is easy to lose scenarios
that describe behavior the change does not touch, and nothing downstream
complains: the resulting spec is still valid, just quieter than it was.

That is not hypothetical. Authoring the 36 deltas for
`establish-delivery-commit-contract` dropped twelve scenarios that survive the
change -- cursor-column gating, the operator-decision case, copy-mode delivery,
the activity-signal sources -- while rewriting the requirements around them.
Every one passed `--strict`. This script found them.

**The audit is title-only.** It compares `#### Scenario:` headings, so it cannot
see a scenario whose title survived while its body was hollowed out, nor a
requirement whose prose lost a normative clause. Those remain a manual-review
concern; this catches the silent-deletion class, not every regression.

Errors, which set a non-zero exit status:

- a change whose deltas disagree about whether they have been synced
- MODIFIED or REMOVED naming a requirement that no live spec holds and no
  other in-flight change supplies (a typo, or a rename that happened upstream)
- ADDED naming a requirement that already exists live in the same capability
  (it should be MODIFIED, or the delta will collide at sync)
- ADDED naming a requirement that exists live in a different capability the
  change does not remove it from, which would leave two definitions of it
- RENAMED whose FROM is absent live, or whose TO already exists

Reports, which print and pass:

- scenarios present live but absent from a MODIFIED delta
- a requirement relocated between capabilities whose text changed in transit
- a delta target that is not live yet because another change has not synced

The reports are reports because each describes something that is often exactly
right: a dropped scenario may describe retired behavior, a move may be
deliberately edited, an ordering constraint may be understood and accepted. The
point is that each should be a decision someone made rather than something that
happened. Read them and confirm every line.

Every one of those checks asks whether a delta agrees with the spec it is about
to modify, so all of them invert once it has modified it. A change is therefore
classified as authored-but-not-yet-synced or as already-synced before any check
runs, and an already-synced change is reported and skipped rather than audited
against a live spec that now contains it. Deltas that disagree with each other
about which state they are in -- some applied, some not -- are an error in their
own right. See `applied_state` for how the state is inferred and why it is not
recorded.

`--citations` runs a different audit, over the corpus rather than over one
change. A requirement cited by name in prose is either live, or supplied by a
change that has not synced yet, or supplied by nothing at all. The middle case
is not a defect -- it is a promise that comes true at sync -- and the third is
permanent. What makes the distinction worth drawing is that archiving a change
without syncing it converts every citation of the first kind into the second,
silently, which is the enforcement gap this mode exists to close.

Citations are recognized by matching a backticked span against the set of known
requirement names, not by the words around it. An earlier checker required the
word "requirement" near the name and missed a real instance phrased as "the
`transport-contracts` capability's `Relay raww operation contract` governs its
request shape". The cost is that a name no requirement ever had -- a typo --
matches nothing and is invisible; this finds citations that stopped resolving,
not citations that were never right.

`--archived` is the gate at the other end. Syncing is required by nothing:
`opsx-archive` offers to sync and archives whichever answer it gets, and once
the deltas are filed no validator reads them again, so a requirement that was
specified and never synced simply never existed. An archive commit is the last
moment the delta and the live spec are still comparable, so it is the only
place the check can be made. It also runs the citation audit, scoped to the
change being filed.

It gates only archives this commit is actually filing, read from the index, so
that amending an archived record later is not gated on a verdict about specs
that have moved on. With nothing staged it therefore checks nothing. To ask the
question anyway -- replaying the gate over history, one archive commit at a
time, where the answer is meaningful because the tree under test is the one
that filed the archive -- add `--unstaged`. Nothing in the hook path passes it,
which is what keeps `pre-commit run --all-files` from turning into a
retrospective sweep of every archive there has ever been.

Usage:  scripts/verify-openspec-deltas.py <change-id> [<change-id> ...]
        scripts/verify-openspec-deltas.py [--quiet] <delta-spec-path> ...
        scripts/verify-openspec-deltas.py [--quiet] --citations
        scripts/verify-openspec-deltas.py [--quiet] --archived <path> ...
        scripts/verify-openspec-deltas.py --archived --unstaged <path> ...

Arguments are change ids, or paths to delta spec files from which the change id
is read. The path form is what the `lint-openspec-deltas` pre-commit hook uses:
pre-commit hands it the staged delta specs, and each change they belong to is
audited once. Paths naming an archived change are skipped in that mode -- an
archived change is a completed record rather than a delta under authorship --
and are what `--archived` reads instead.

Run from anywhere; paths resolve from this script's own location. Run it after
`openspec validate <change-id> --strict` passes and before requesting review.

The per-change hook covers every commit that touches a delta spec. It cannot
cover the archive-order case: when another change archives into the same
requirement, the live spec moves beneath an already-committed delta and the drop
set changes with no delta file touched, so no commit fires the hook. In that
case run the per-change form by hand against the post-archive live spec:

    scripts/verify-openspec-deltas.py <change-id>

That is the first usage above and has nothing to do with `--archived`, whose
`--unstaged` flag governs replaying the ARCHIVE gate over history. The two
by-hand cases are easy to conflate because both are described as running the
script manually; they audit different things.
"""

import difflib
import re
import subprocess
import sys
from collections import namedtuple
from pathlib import Path

REQUIREMENT = "### Requirement:"
SCENARIO = "#### Scenario:"

# One requirement as it appears in a spec or a delta: its name, the titles of
# the scenarios beneath it, and the verbatim text of the whole block.
#
# The block text is retained because several properties of a delta cannot be
# decided from names alone -- whether a relocated requirement arrived intact,
# and whether a MODIFIED delta has already been applied to live. Both compare
# text, and both would be impossible against a name-and-scenario summary.
Requirement = namedtuple("Requirement", "name scenarios text")

# A requirement block runs to the next requirement header or the next section
# header, whichever comes first.
REQUIREMENT_HEADER = re.compile(r"^### Requirement:[ \t]*(.+?)[ \t]*$", re.M)
BLOCK_STOP = re.compile(r"^(?:### Requirement:|## )", re.M)
SECTION_HEADER = re.compile(r"^## ([A-Z]+) Requirements\s*$", re.M)

# A markdown thematic break: three or more of the same dash, asterisk or
# underscore on a line of their own.
THEMATIC_BREAK = re.compile(r"^\s*([-*_])\1{2,}\s*$")

# OpenSpec requires a requirement statement to say SHALL or MUST, so these
# words mark the sentences that carry the obligation.
NORMATIVE_WORD = re.compile(r"\b(?:SHALL|MUST)\b")

# Candidate planning homes, relative to the repository root, in priority order.
#
# `openspec/` at the root is the canonical location -- it is what `openspec init`
# creates. This repository keeps the real tree under `documentation/architecture/`
# for historical reasons and reaches it through a root `openspec` symlink, so the
# first candidate resolves correctly here too.
#
# The second candidate is not redundant: that symlink is untracked and listed in
# `.git/info/exclude`, so it exists only where somebody created it by hand. A
# fresh clone or a new worktree has no `openspec/` at all, which is why resolving
# these paths by shelling out to `openspec` fails there. Deriving them from this
# script's own location works in every case.
PLANNING_HOMES = (Path("openspec"), Path("documentation/architecture/openspec"))

# The change id sits between `changes/` and `specs/` in a delta spec path.
DELTA_PATH = re.compile(r"/changes/([^/]+)/specs/")

# An archived change's directory is its id under a date prefix. Any file in it
# identifies the change, not only a delta spec: the archive commit is a
# directory move, so it may stage a proposal or a tasks file and nothing else.
ARCHIVE_PATH = re.compile(r"/changes/archive/([^/]+)/")

# An archived change lives under `changes/archive/<date>-<id>/`, so its id
# segment reads as the literal `archive`.
ARCHIVE_SEGMENT = "archive"


def repository_root():
    """Return the repository root, derived from this script's own location."""
    return Path(__file__).resolve().parents[1]


def planning_home():
    """Return the OpenSpec planning home directory, or exit if there is none."""
    repo_root = repository_root()
    home = next(
        (
            (repo_root / candidate).resolve()
            for candidate in PLANNING_HOMES
            if (repo_root / candidate).is_dir()
        ),
        None,
    )
    if home is None:
        looked = " and ".join(str(repo_root / c) for c in PLANNING_HOMES)
        sys.exit(f"cannot locate the OpenSpec planning home; looked for {looked}")
    return home


def resolve_paths(change_id):
    """Return (change_specs_dir, live_specs_dir), or None if the change is absent."""
    home = planning_home()
    change_root = home / "changes" / change_id
    if not change_root.is_dir():
        return None
    return change_root / "specs", home / "specs"


def parse_requirements(body):
    """Return {name: Requirement} for one span of markdown."""
    out = {}
    for header in REQUIREMENT_HEADER.finditer(body):
        name = header.group(1)
        stop = BLOCK_STOP.search(body, header.end())
        text = body[header.start():stop.start() if stop else len(body)]
        scenarios = [
            line[len(SCENARIO):].strip()
            for line in text.splitlines()
            if line.startswith(SCENARIO)
        ]
        # A name repeated within one span is malformed markdown rather than a
        # merge; keep the first block and accumulate the scenarios, so the
        # retention comparison sees every scenario that would be at risk.
        if name in out:
            out[name] = out[name]._replace(scenarios=out[name].scenarios + scenarios)
        else:
            out[name] = Requirement(name, scenarios, text)
    return out


def parse(path):
    """Return {name: Requirement} for one spec file."""
    if not path.is_file():
        return {}
    return parse_requirements(path.read_text())


def parse_delta(path):
    """Return ({operation: {name: Requirement}}, [(FROM|TO, name), ...]).

    Splits on `## <OP> Requirements` headers so each requirement is attributed
    to the operation it appears under. A RENAMED block carries FROM:/TO: lines
    rather than requirement headers, so its pairs are returned separately.
    """
    text = path.read_text()
    blocks = re.split(SECTION_HEADER, text)
    result, pairs = {}, []
    for index in range(1, len(blocks), 2):
        operation, body = blocks[index], blocks[index + 1]
        section = result.setdefault(operation, {})
        section.update(parse_requirements(body))
        if operation == "RENAMED":
            pairs.extend(
                re.findall(
                    r"^\s*-\s*(FROM|TO):\s*`?(?:### Requirement:)?\s*([^`\n]+?)`?\s*$",
                    body,
                    flags=re.M,
                )
            )
    return result, pairs


def read_capability(live_specs, capability):
    """Return {name: Requirement} merged across every file of one capability.

    OpenSpec declares the specs artifact as `specs/**/*.md`, so a capability may
    be split across several files rather than a single `spec.md`. Merge every
    file under the capability directory, or a requirement defined in a sibling
    file reads as missing. Filtering by basename instead would skip a nested
    `<capability>/parts/spec.md` too.
    """
    root_spec = live_specs / capability / "spec.md"
    merged = parse(root_spec)
    for extra in sorted((live_specs / capability).rglob("*.md")):
        if extra == root_spec:
            continue
        for name, requirement in parse(extra).items():
            if name in merged:
                merged[name] = merged[name]._replace(
                    scenarios=merged[name].scenarios + requirement.scenarios
                )
            else:
                merged[name] = requirement
    return merged


def read_change(change_specs):
    """Return {capability: ({operation: {name: Requirement}}, pairs)}.

    Delta files under one capability directory are merged, so a capability split
    across several files audits as one unit.
    """
    by_capability = {}
    for delta_path in sorted(change_specs.rglob("*.md")):
        capability = delta_path.relative_to(change_specs).parts[0]
        sections, pairs = parse_delta(delta_path)
        merged_sections, merged_pairs = by_capability.setdefault(capability, ({}, []))
        for operation, requirements in sections.items():
            merged_sections.setdefault(operation, {}).update(requirements)
        merged_pairs.extend(pairs)
    return by_capability


# A citation is a requirement name written in backticks somewhere in prose.
#
# Matching is by NAME against the set of requirement names that exist anywhere
# in the corpus, and deliberately not by the phrasing around the name. An
# earlier checker keyed on a backticked name followed by the word "requirement"
# and missed the instance found in review -- `transport-abstraction` read "the
# `transport-contracts` capability's `Relay raww operation contract` governs its
# request shape", with no "requirement" anywhere near it. Citations take too
# many phrasings to enumerate; requirement names are distinctive enough to
# recognize on their own.
#
# The cost of that choice is recall in the other direction: a citation naming a
# requirement that never existed anywhere -- a typo, or a name invented in
# prose -- matches nothing and is invisible here. This finds citations that
# have stopped resolving, not every citation that was never right.
CITATION = re.compile(r"`([^`\n]{4,120})`")

# Where a citation may be written. Archived changes are excluded: an archived
# change is a record of what was decided, and a name it cites that never
# reached live is history rather than a defect to repair.
CORPUS_GLOBS = ("documentation/**/*.md", "src/**/*.md", "src/**/*.rs", "*.md")


def delta_names(change_root):
    """Return {requirement_name: capability} a change would bring into being.

    Only ADDED and the TO half of a RENAMED create a requirement that does not
    exist yet. MODIFIED does not: it edits a requirement that is already live,
    so a MODIFIED delta naming something absent is a broken delta rather than a
    promise of one. Counting MODIFIED here made this function answer a
    different question than its two callers ask -- it let a change excuse
    another change's missing target merely by editing the same name, and it
    made a citation to a name nothing creates read as merely early.

    RENAMED was missed in the other direction: its TO names entered only by
    accident, because a rename is usually written alongside a MODIFIED block
    keyed by the new name. Dropping MODIFIED without adding TO explicitly would
    have taken real renames out of the citation universe with it.
    """
    found = {}
    specs = change_root / "specs"
    if not specs.is_dir():
        return found
    for path in sorted(specs.rglob("*.md")):
        capability = path.relative_to(specs).parts[0]
        sections, pairs = parse_delta(path)
        for name in sections.get("ADDED", {}):
            found.setdefault(name, capability)
        for _, to_name in rename_pairs(pairs):
            found.setdefault(to_name, capability)
    return found


def name_universe(home):
    """Return (live, inflight, archived) requirement-name maps.

    `live` maps a name to the capability holding it. The other two map a name
    to the changes that would supply it, which is what separates a citation
    that is merely early from one that can never resolve.
    """
    live, inflight, archived = {}, {}, {}
    for spec in sorted((home / "specs").rglob("*.md")):
        capability = spec.relative_to(home / "specs").parts[0]
        for name in parse(spec):
            live.setdefault(name, capability)
    for change_root in sorted((home / "changes").iterdir()):
        if not change_root.is_dir() or change_root.name == ARCHIVE_SEGMENT:
            continue
        for name, capability in delta_names(change_root).items():
            inflight.setdefault(name, []).append((change_root.name, capability))
    archive_dir = home / "changes" / ARCHIVE_SEGMENT
    for change_root in sorted(archive_dir.iterdir()) if archive_dir.is_dir() else []:
        if not change_root.is_dir():
            continue
        for name, capability in delta_names(change_root).items():
            archived.setdefault(name, []).append((change_root.name, capability))
    return live, inflight, archived


def sourcing_changes(sources):
    """Return the change ids from a name-universe source list, in order."""
    seen = []
    for change_id, _ in sources:
        if change_id not in seen:
            seen.append(change_id)
    return seen


def citation_corpus(root, home):
    """Return the paths that may carry a citation, deduplicated and ordered.

    `documentation/**` reaches the archive tree, so archived changes are
    filtered out here rather than merely omitted from the globs. Leaving them
    in buries the live findings: the archive holds 46 citations to requirements
    that were renamed or removed by changes that came after, every one of them
    an accurate record of what was true when it was written.
    """
    archive = home / "changes" / ARCHIVE_SEGMENT
    paths, seen = [], set()
    for glob in CORPUS_GLOBS:
        for path in sorted(root.glob(glob)):
            if not path.is_file() or path in seen or archive in path.parents:
                continue
            seen.add(path)
            paths.append(path)
    for change_root in sorted((home / "changes").iterdir()):
        if not change_root.is_dir() or change_root.name == ARCHIVE_SEGMENT:
            continue
        for path in sorted(change_root.rglob("*.md")):
            if path not in seen:
                seen.add(path)
                paths.append(path)
    return paths


def citing_change(path, home):
    """Return the change id whose directory holds this file, or None."""
    changes = home / "changes"
    try:
        parts = path.relative_to(changes).parts
    except ValueError:
        return None
    return parts[0] if parts and parts[0] != ARCHIVE_SEGMENT else None


DATED_PREFIX = re.compile(r"^\d{4}-\d{2}-\d{2}-")


def cites_its_archive(document, sources):
    """Whether the citing document names an archived change it points into.

    A dangling citation is a defect because the reader is sent to a requirement
    that no longer exists and given nowhere to look. A document that names the
    archive alongside the requirement has already answered that: it is
    discussing a requirement's absence rather than depending on its presence,
    which is what a design note explaining a superseded rule does.

    The dated directory name and the bare change id both count, since a prose
    reference to a change does not always carry its archive date.

    Matching is bounded on both sides rather than by substring: change ids
    routinely extend one another, so a plain `in` test would let a mention of
    `add-relay-stream-hello-transport-mvp` excuse a citation stranded by
    `add-relay`.
    """
    for source in sources:
        for candidate in (source, DATED_PREFIX.sub("", source)):
            if re.search(rf"(?<![\w-]){re.escape(candidate)}(?![\w-])", document):
                return True
    return False


def classify_citation(name, owner, live, inflight, archived, document=""):
    """Return (verdict, sources) for one cited name, or None when it resolves.

    `owner` is the change whose directory holds the citing file, if any. A
    change citing a requirement it introduces itself is self-consistent and
    reports nothing; only a citation from outside is waiting on somebody else.

    `document` is the full text of the citing file, consulted only to let a
    document that names its own archive discuss a departed requirement.
    """
    if name in live:
        return None
    if name in inflight:
        sources = sourcing_changes(inflight[name])
        return None if owner in sources else ("PENDING", sources)
    if name in archived:
        sources = sourcing_changes(archived[name])
        return None if cites_its_archive(document, sources) else ("DANGLING", sources)
    return None


def check_citations(quiet, only_changes=None):
    """Audit requirement citations across the corpus.

    Returns (errors, pending) where pending lists citations that resolve only
    into a change that has not synced yet. Those are not defects: they are
    promises that come true at sync, which is exactly why archiving a change
    without syncing it is the moment they become permanent.

    `only_changes` narrows the errors to citations the named changes are
    responsible for. The archive gate passes the change it is archiving,
    because an archive commit must answer for the citations it strands and not
    for every citation the corpus already carried: run unscoped against an
    older commit, this reports two hundred historical errors and gates nothing
    anybody can act on.
    """
    home = planning_home()
    root = repository_root()
    live, inflight, archived = name_universe(home)

    errors, pending = [], []
    for path in citation_corpus(root, home):
        owner = citing_change(path, home)
        text = path.read_text()
        for match in CITATION.finditer(text):
            name = match.group(1).strip()
            verdict = classify_citation(name, owner, live, inflight, archived, text)
            if verdict is None:
                continue
            kind, sources = verdict
            line = text.count("\n", 0, match.start()) + 1
            where = f"{path.relative_to(root)}:{line}"
            if kind == "PENDING":
                pending.append((where, name, sources))
            elif only_changes is None or any(s in only_changes for s in sources):
                errors.append(
                    f"{where}: '{name}' resolves to no live requirement; it was "
                    f"last named by archived {', '.join(sources)}"
                )

    if pending and not quiet:
        print("\n=== citations awaiting a sync")
        for where, name, sources in pending:
            print(f"\n  {where}")
            print(f"     PENDING  '{name}' <- {', '.join(sources)}")
    return errors, pending


def change_ids(arguments):
    """Return the unique change ids named by a mix of ids and delta spec paths.

    A path that names no change is skipped rather than treated as an id: the
    hook's file filter should exclude those, and inventing a change id out of an
    unrelated path would report it as missing and fail the commit.
    """
    ids = []
    for argument in arguments:
        if "/" in argument:
            found = DELTA_PATH.search(argument)
            if found is None:
                continue
            change_id = found.group(1)
        else:
            change_id = argument
        if change_id == ARCHIVE_SEGMENT or change_id in ids:
            continue
        ids.append(change_id)
    return ids


def flattened(text):
    """Return text with whitespace collapsed and markdown emphasis removed.

    Both sides of every clause comparison go through this, so a requirement
    that is only rewrapped, reindented, or emphasised differently compares
    equal. Stripping the markers is not cosmetic: a sentence ending `one.**`
    has no whitespace after its period, so a splitter that looks for one runs
    two sentences together and then fails to match live whenever the *second*
    one changed. That produced a false positive convincing enough to be
    reported as a finding before it was checked.
    """
    return " ".join(re.sub(r"[*`_]+", "", text).split())


def prose_of(requirement):
    """Return the requirement's non-list text, flattened.

    Both the clauses extracted from a delta and the live text they are looked
    up in must come through here. Dropping list lines joins whatever sat on
    either side of a list, so a clause extracted from stripped text can only
    be found in text stripped the same way -- searching the full live text for
    it flagged 72 of 96 archives instead of 3.
    """
    kept = "\n".join(
        line for line in requirement.text.splitlines()
        if not line.lstrip().startswith(("-", "*", "+"))
    )
    return flattened(kept)


def normative_clauses(requirement):
    """Return the requirement's SHALL/MUST prose sentences, flattened.

    These are the sentences that carry the obligation, so they are what a
    change is for and what is lost if its delta never reaches live. Comparing
    them survives the drift that defeats comparing whole texts: rewrapping and
    reindenting vanish under flattening, and a later change adding prose
    leaves the existing clauses where they were.

    List items are excluded, and that exclusion is what makes this usable as a
    gate. Bullets rarely end in a period, so a list flattens into one enormous
    pseudo-sentence that swallows the prose after it; any edit to any item then
    makes the whole block compare unequal. Two archives were flagged that way
    -- for a changed parameter list, not a lost obligation -- which is the kind
    of noise that teaches people to bypass a gate. The cost is narrower reach:
    an obligation stated only inside a bullet is not compared.
    """
    sentences = re.split(r"(?<=\.)\s+", prose_of(requirement))
    return [s for s in sentences if NORMATIVE_WORD.search(s)]


def rename_pairs(pairs):
    """Yield (from_name, to_name) for each well-formed RENAMED pair.

    Well-formed means alternating FROM/TO with both names non-empty. Whether
    the pair is *valid* -- FROM live, TO not yet live -- is a separate question
    answered against a particular capability's live spec by `check_renames`,
    which is the one place that reports it. Callers that only need to know what
    a change intends to rename use this.
    """
    for index in range(0, len(pairs) - 1, 2):
        (from_kind, from_name), (to_kind, to_name) = pairs[index], pairs[index + 1]
        if from_kind == "FROM" and to_kind == "TO" and from_name and to_name:
            yield from_name, to_name


def normalized(requirement):
    """Return a requirement's block text with document furniture removed.

    Sync copies a delta block into the live spec verbatim, so equality is
    otherwise exact. Two things are not part of the requirement and must not
    count as differences between two copies of it: the blank lines each file
    happens to carry around it, and a trailing horizontal rule.

    A block runs to the next requirement or section header, so a rule written
    between requirements is absorbed into the one above it. Live specs use them
    as separators and deltas do not, which made an archived change read as
    unsynced when the only difference between its delta and live was four
    dashes that belonged to neither.
    """
    lines = [line.rstrip() for line in requirement.text.strip().splitlines()]
    while lines and THEMATIC_BREAK.match(lines[-1]):
        lines.pop()
        while lines and not lines[-1]:
            lines.pop()
    return "\n".join(lines)


def applied_state(by_capability, live_by_capability):
    """Classify a change as APPLIED, PENDING, or MIXED against the live specs.

    A delta has two normal states -- authored but not yet synced, and synced --
    and every existence check in this script inverts between them. Read without
    that distinction, a correctly synced change reports one error per delta:
    its REMOVED targets are gone (as intended) and its ADDED requirements are
    present (as intended). Twelve such errors on `extract-raww-verb-capability`
    immediately after its sync are what motivated this.

    The state is inferred rather than recorded. `opsx-sync` edits live specs by
    hand, so a marker written at sync time would be one more thing to forget;
    inferring from the artifacts is self-correcting and needs no new discipline.

    Every operation contributes evidence, and they are weighed together. An
    earlier version let ADDED and REMOVED names decide alone whenever a change
    had any, which silently discarded the rest: a change with one synced
    removal and one unsynced modification read as fully applied, and the
    archive gate would have accepted it while the modification was lost.

    - REMOVED is live before sync and absent after.
    - ADDED is the reverse.
    - MODIFIED is live in both states, so the signal is whether the scenarios
      AND the normative prose clauses the delta introduces are present. Scenario
      headings alone left the case that matters most unguarded: a delta that
      rewrites a SHALL without touching a heading is exactly the edit a change
      exists to make, and exactly what is lost if it never syncs.
    - RENAMED leaves FROM gone and TO live. Only the two together are evidence;
      either half alone is ambiguous.

    Comparing MODIFIED texts for equality was tried first and is wrong. Live
    drifts away from an applied delta for reasons that have nothing to do with
    syncing -- a later change edits the same requirement, or the paragraph is
    rewrapped -- and against real archive commits it produced false verdicts on
    two of twelve, each of which would have blocked a correct archive.
    Containment survives both: rewrapping does not move a scenario, and a live
    requirement that has since gained more still holds what the delta added.

    Returns (state, evidence). APPLIED, PENDING, MIXED when the evidence
    disagrees with itself, and SATISFIED when there is none.

    SATISFIED is not ignorance, which is why it is not called UNKNOWN. It means
    every requirement, scenario and normative clause the delta names is already
    present in live. Whether this change put it there is unknowable and also
    immaterial: nothing the delta asserts would be lost by filing it. That is
    the invariant the archive gate enforces, stated exactly.

    Every correctly synced MODIFIED-only change lands here by construction --
    once its edits are live there is nothing left for it to introduce -- so
    blocking on SATISFIED would block 25 of this repository's 96 archives. It
    is a pass.

    The residual hole is a delta whose purpose is DELETION: if it removes an
    obligation and never syncs, live retains the old clause, the delta's
    remaining clauses are all present, and it reports SATISFIED. That case is
    not detectable here, because live holding more than the delta is also what
    ordinary later drift looks like.
    """
    applied, pending = [], []
    for capability, (delta, pairs) in sorted(by_capability.items()):
        live = live_by_capability[capability]
        # Each line says which state it is evidence for, not merely what is
        # true. "REMOVED 'X' is absent from live" is evidence the change HAS
        # synced, while the identical phrase about an ADDED requirement is
        # evidence it has NOT; a reader should not have to invert the sense
        # per operation to read the list.
        for name in delta.get("REMOVED", {}):
            gone = name not in live
            (applied if gone else pending).append(
                f"synced: {capability}: REMOVED '{name}' is gone from live"
                if gone
                else f"unsynced: {capability}: REMOVED '{name}' is still live"
            )
        for name in delta.get("ADDED", {}):
            there = name in live
            (applied if there else pending).append(
                f"synced: {capability}: ADDED '{name}' is live"
                if there
                else f"unsynced: {capability}: ADDED '{name}' never reached live"
            )
        for name, requirement in delta.get("MODIFIED", {}).items():
            before = live.get(name)
            if before is None:
                continue
            introduced = [
                scenario
                for scenario in requirement.scenarios
                if scenario not in before.scenarios
            ]
            # Scenario headings alone leave the case that matters most
            # unguarded: a delta that rewrites a normative clause without
            # touching a heading. That is precisely the edit a change exists
            # to make, and precisely what is lost if the delta never syncs.
            live_text = prose_of(before)
            rewritten = [
                clause
                for clause in normative_clauses(requirement)
                if clause not in live_text
            ]
            # MODIFIED yields a one-sided signal only. A scenario or clause
            # counts as introduced precisely when live lacks it, so once the
            # delta has synced there is nothing left to introduce and the
            # evidence goes quiet. It can show that a delta has NOT landed,
            # never that it has.
            if introduced or rewritten:
                missing = []
                if introduced:
                    missing.append(f"{len(introduced)} scenario(s)")
                if rewritten:
                    missing.append(f"{len(rewritten)} normative clause(s)")
                pending.append(
                    f"unsynced: {capability}: MODIFIED '{name}' introduces "
                    f"{' and '.join(missing)} the live spec lacks"
                )
        for from_name, to_name in rename_pairs(pairs):
            # A completed rename leaves the FROM name gone and the TO name
            # live. Exactly that shape is evidence of a sync; every other shape
            # is evidence against one, and none of them is merely ambiguous.
            #
            # Treating the other shapes as no-evidence let an unsynced rename
            # archive whenever the TO name happened to exist for an unrelated
            # reason: FROM was still live -- the old name never removed, the
            # rename never performed -- and the gate passed it because the two
            # signals cancelled. The old name being live is the whole thing a
            # rename removes, so it decides this on its own.
            if from_name not in live and to_name in live:
                applied.append(
                    f"synced: {capability}: RENAMED '{from_name}' -> "
                    f"'{to_name}' is reflected live"
                )
            elif from_name in live and to_name in live:
                pending.append(
                    f"unsynced: {capability}: RENAMED '{from_name}' -> "
                    f"'{to_name}' left both names live, so the rename never "
                    "removed the old one"
                )
            elif from_name in live:
                pending.append(
                    f"unsynced: {capability}: RENAMED '{from_name}' -> "
                    f"'{to_name}' has not been applied"
                )
            else:
                pending.append(
                    f"unsynced: {capability}: RENAMED '{from_name}' -> "
                    f"'{to_name}' left neither name live"
                )

    if applied and pending:
        # Unsynced first: on a large change the applied half runs to dozens of
        # lines that say only that the parts which worked, worked.
        return "MIXED", pending + applied
    if applied:
        return "APPLIED", []
    if pending:
        return "PENDING", pending
    return "SATISFIED", []


# Each case is `(name, delta_sections, rename_pairs, live, expected_state)`.
#
# The classifier decides which half of the script's checks apply, so getting it
# wrong is worse than not having it: a change misread as APPLIED skips the
# retention report entirely, and the archive gate accepts it. Real changes
# exercise only a few of these branches, so the rest are covered here or
# nowhere.
def _requirement(name, text, scenarios=()):
    body = f"### Requirement: {name}\n\n{text}\n"
    for scenario in scenarios:
        body += f"\n#### Scenario: {scenario}\n\n- **WHEN** a thing\n- **THEN** another\n"
    return Requirement(name, list(scenarios), body)


def _renamed(from_name, to_name):
    return [("FROM", from_name), ("TO", to_name)]


SELFTEST_CASES = [
    (
        "authored, not yet synced",
        {"ADDED": {"New": _requirement("New", "a")},
         "REMOVED": {"Old": _requirement("Old", "b")}},
        [],
        {"Old": _requirement("Old", "b")},
        "PENDING",
    ),
    (
        "synced",
        {"ADDED": {"New": _requirement("New", "a")},
         "REMOVED": {"Old": _requirement("Old", "b")}},
        [],
        {"New": _requirement("New", "a")},
        "APPLIED",
    ),
    (
        "half-synced: the addition landed, the removal did not",
        {"ADDED": {"New": _requirement("New", "a")},
         "REMOVED": {"Old": _requirement("Old", "b")}},
        [],
        {"New": _requirement("New", "a"), "Old": _requirement("Old", "b")},
        "MIXED",
    ),
    # Evidence from different operations has to be weighed together. Letting
    # ADDED and REMOVED decide alone read this as fully applied, and the
    # archive gate accepted it while the modification was lost.
    (
        "a synced removal alongside a modification that never landed",
        {"REMOVED": {"Gone": _requirement("Gone", "b")},
         "MODIFIED": {"Kept": _requirement("Kept", "after", ["Old", "New"])}},
        [],
        {"Kept": _requirement("Kept", "before", ["Old"])},
        "MIXED",
    ),
    (
        "modified only, the scenario it introduces is not live yet",
        {"MODIFIED": {"Same": _requirement("Same", "after", ["Old", "New"])}},
        [],
        {"Same": _requirement("Same", "before", ["Old"])},
        "PENDING",
    ),
    (
        "modified only, one of two has not landed",
        {"MODIFIED": {"One": _requirement("One", "after", ["A"]),
                      "Two": _requirement("Two", "after", ["B"])}},
        [],
        {"One": _requirement("One", "after", ["A"]),
         "Two": _requirement("Two", "before", [])},
        "PENDING",
    ),
    # A MODIFIED delta can show that it has NOT landed and never that it has:
    # a scenario counts as introduced exactly when live lacks it, so an applied
    # delta has nothing left to introduce and looks like one editing only
    # prose. All three of these are indistinguishable, and SATISFIED says so
    # rather than guessing. Live has also drifted in the last two -- rewrapped,
    # and grown a scenario of its own -- both of which comparing texts for
    # equality wrongly called unsynced on real archive commits.
    (
        "modified only, already synced",
        {"MODIFIED": {"Same": _requirement("Same", "after", ["Old", "New"])}},
        [],
        {"Same": _requirement("Same", "after", ["Old", "New"])},
        "SATISFIED",
    ),
    (
        "modified only, synced and since rewrapped",
        {"MODIFIED": {"Same": _requirement("Same", "one line", ["A"])}},
        [],
        {"Same": _requirement("Same", "one\nline", ["A"])},
        "SATISFIED",
    ),
    (
        "modified only, synced and live has since gained a scenario",
        {"MODIFIED": {"Same": _requirement("Same", "after", ["A"])}},
        [],
        {"Same": _requirement("Same", "after", ["A", "Added Later"])},
        "SATISFIED",
    ),
    (
        "modified only, introducing no scenario is not evidence of anything",
        {"MODIFIED": {"Same": _requirement("Same", "after", ["A"])}},
        [],
        {"Same": _requirement("Same", "before", ["A"])},
        "SATISFIED",
    ),
    # A normative clause rewritten without touching a scenario heading. This is
    # the edit a change most often exists to make, and checking headings alone
    # let it through the archive gate to be lost.
    (
        "modified only, a normative clause the live spec does not have",
        {"MODIFIED": {"Rule": _requirement(
            "Rule", "The relay MUST use the new rule.", ["A case"])}},
        [],
        {"Rule": _requirement("Rule", "The relay MUST use the old rule.", ["A case"])},
        "PENDING",
    ),
    (
        "modified only, that clause has landed and live was since rewrapped",
        {"MODIFIED": {"Rule": _requirement(
            "Rule", "The relay MUST use the new rule.", ["A case"])}},
        [],
        {"Rule": _requirement(
            "Rule", "The relay MUST use the\nnew rule.", ["A case"])},
        "SATISFIED",
    ),
    (
        "modified only, that clause has landed and live gained prose after it",
        {"MODIFIED": {"Rule": _requirement(
            "Rule", "The relay MUST use the new rule.", ["A case"])}},
        [],
        {"Rule": _requirement(
            "Rule", "The relay MUST use the new rule. A later change added this.",
            ["A case"])},
        "SATISFIED",
    ),
    # A parameter list that gained an item, with the surrounding obligations
    # unchanged. Comparing list text flagged two real archives for this, which
    # is churn rather than a lost obligation -- so list lines are dropped from
    # both sides, and this case must stay a pass.
    (
        "modified only, a list item differs but the obligations do not",
        {"MODIFIED": {"Tool": _requirement(
            "Tool",
            "look SHALL support:\n\n- target_session\n- lines\n\n"
            "Routing context SHALL be inferred from the suffix.",
            ["A case"])}},
        [],
        {"Tool": _requirement(
            "Tool",
            "look SHALL support:\n\n- target_session\n- lines\n- bundle_name\n\n"
            "Routing context SHALL be inferred from the suffix.",
            ["A case"])},
        "SATISFIED",
    ),
    # A rename carries its own evidence, in neither the ADDED nor the REMOVED
    # section. Ignoring it made a correctly synced pure rename read as PENDING,
    # so the archive gate refused it.
    (
        "pure rename, synced",
        {},
        _renamed("Old Name", "New Name"),
        {"New Name": _requirement("New Name", "a", ["A"])},
        "APPLIED",
    ),
    (
        "pure rename, not yet synced",
        {},
        _renamed("Old Name", "New Name"),
        {"Old Name": _requirement("Old Name", "a", ["A"])},
        "PENDING",
    ),
    # Only one shape is a completed rename. The other three are evidence
    # against one, not absence of evidence -- reading them as absence let an
    # unsynced rename archive whenever the new name existed for an unrelated
    # reason, with the old name still sitting live beside it.
    (
        "pure rename, neither name live",
        {},
        _renamed("Old Name", "New Name"),
        {},
        "PENDING",
    ),
    (
        "pure rename, both names live so the old one was never removed",
        {},
        _renamed("Old Name", "New Name"),
        {"Old Name": _requirement("Old Name", "a"),
         "New Name": _requirement("New Name", "a")},
        "PENDING",
    ),
]


# Each case is `(name, cited_name, citing_change, expected_verdict)` against the
# fixed universe below. The DANGLING branch reports nothing on today's corpus,
# so without these it would be an assertion of absence from a check that has
# never once fired.
#
# `Amended Requirement` and `Restored Requirement` are the ordinary case rather
# than a curiosity: a MODIFIED delta names a requirement that is already live,
# and an ADDED one stays named by its change after that change archives. Both
# are live AND named by a change, so a universe without them cannot tell
# whether the live check is doing anything.
CITATION_UNIVERSE = (
    {
        "Live Requirement": "some-capability",
        "Amended Requirement": "some-capability",
        "Restored Requirement": "some-capability",
    },
    {
        "Arriving Requirement": [("some-change", "some-capability")],
        "Amended Requirement": [("some-change", "some-capability")],
    },
    {
        "Departed Requirement": [("2026-01-01-archived-change", "some-capability")],
        "Restored Requirement": [("2026-01-01-archived-change", "some-capability")],
    },
)

# Each case is `(name, cited, owner, document, expected_verdict)`.
CITATION_CASES = [
    ("a live requirement", "Live Requirement", None, "", None),
    ("a name nobody has ever used", "Invented Requirement", None, "", None),
    ("a live requirement an in-flight change also modifies",
     "Amended Requirement", None, "", None),
    ("a live requirement an archived change introduced",
     "Restored Requirement", None, "", None),
    ("an in-flight requirement cited from outside", "Arriving Requirement",
     None, "", "PENDING"),
    ("an in-flight requirement cited by another change", "Arriving Requirement",
     "other-change", "", "PENDING"),
    ("an in-flight requirement cited by the change adding it",
     "Arriving Requirement", "some-change", "", None),
    ("a requirement no change will restore", "Departed Requirement",
     None, "", "DANGLING"),
    ("a departed requirement cited from within a change",
     "Departed Requirement", "some-change", "", "DANGLING"),
    ("a departed requirement whose document names the dated archive",
     "Departed Requirement", None, "superseded by 2026-01-01-archived-change",
     None),
    ("a departed requirement whose document names the archive undated",
     "Departed Requirement", None, "superseded by archived-change", None),
    ("a departed requirement whose document names a different archive",
     "Departed Requirement", None, "superseded by 2026-01-01-other-change",
     "DANGLING"),
    ("a departed requirement whose document names a longer id sharing a prefix",
     "Departed Requirement", None, "see archived-change-two", "DANGLING"),
]


# Each case is `(name, by_capability, live_owners, expected_error_count)`.
# All three short-circuit before any live spec is read, so they need no
# filesystem. They cover the two ways an ADDED requirement can collide with a
# live one in another capability, neither of which the per-capability collision
# check can see.
MOVE_CASES = [
    (
        "a move with one source and one destination",
        {"source": ({"REMOVED": {"Moved": _requirement("Moved", "a")}}, []),
         "dest": ({"ADDED": {"Moved": _requirement("Moved", "a")}}, [])},
        {"Moved": "source"},
        0,
    ),
    (
        "one source claimed by two destinations",
        {"source": ({"REMOVED": {"Moved": _requirement("Moved", "a")}}, []),
         "dest_a": ({"ADDED": {"Moved": _requirement("Moved", "a")}}, []),
         "dest_b": ({"ADDED": {"Moved": _requirement("Moved", "a")}}, [])},
        {"Moved": "source"},
        1,
    ),
    (
        "an addition duplicating a live requirement it does not remove",
        {"dest": ({"ADDED": {"Moved": _requirement("Moved", "a")}}, [])},
        {"Moved": "source"},
        1,
    ),
]


def run_selftest():
    """Return a list of failure descriptions; empty when the classifiers work."""
    failures = []
    for name, by_capability, live_owners, expected in MOVE_CASES:
        errors, _, _ = check_moves(
            "selftest", by_capability, Path("/nonexistent"), live_owners, True
        )
        if len(errors) != expected:
            failures.append(
                f"move case {name!r} reported {len(errors)} errors, "
                f"expected {expected}"
            )
    for name, delta, pairs, live, expected in SELFTEST_CASES:
        state, _ = applied_state({"c": (delta, pairs)}, {"c": live})
        if state != expected:
            failures.append(
                f"applied-state case {name!r} classified {state}, expected {expected}"
            )
    for name, cited, owner, document, expected in CITATION_CASES:
        verdict = classify_citation(cited, owner, *CITATION_UNIVERSE, document)
        actual = verdict[0] if verdict else None
        if actual != expected:
            failures.append(
                f"citation case {name!r} classified {actual}, expected {expected}"
            )
    return failures


# A relocated requirement's text is shown as a diff rather than a verdict,
# capped so that a deliberate rewrite does not bury the run. A transit casualty
# -- a truncation, a lost scenario, a paragraph that did not survive a
# copy-paste -- is a short diff; a long one means the move is also an edit, and
# the author knows whether that was the intent.
MOVE_DIFF_LINES = 40


def check_moves(change_id, by_capability, live_specs, live_owners, quiet):
    """Compare each relocated requirement against the text it is leaving.

    A move between capabilities is a REMOVED delta in the source and an ADDED
    delta in the destination, and nothing compares the two halves. The retention
    check audits within a capability, so a move that rewrites, truncates or
    drops scenarios in transit passes it; `openspec validate --strict` sees two
    well-formed deltas and is satisfied.

    The comparison is against the live source text rather than against the
    REMOVED delta, because a REMOVED block carries a `**Reason**` and not the
    requirement it removes. That makes this a pre-sync check by construction:
    once the change syncs, the text it moved away from is gone. It is also
    exactly when the check is wanted.

    Detection matches on name, so a move that also renames is invisible here and
    should be authored as a removal plus an addition, with the rename called out.
    """
    errors, moved, altered = [], 0, 0
    removed_from = {}
    for capability, (delta, _) in by_capability.items():
        for name in delta.get("REMOVED", {}):
            removed_from.setdefault(name, []).append(capability)

    sources, destinations = {}, {}
    for capability, (delta, _) in sorted(by_capability.items()):
        for name, arriving in delta.get("ADDED", {}).items():
            origin = live_owners.get(name)
            # Absent live: a genuinely new requirement. Live in this same
            # capability: the collision the ADDED check already reports.
            if origin is None or origin == capability:
                continue
            if origin not in removed_from.get(name, []):
                errors.append(
                    f"{capability}: ADDED '{name}' already exists live in "
                    f"{origin}, which this change does not remove it from -- "
                    "two capabilities would define the same requirement"
                )
                continue
            # One requirement leaving one capability can arrive in only one.
            # Two ADDED destinations sharing a source both look like a
            # well-formed move from their own side, and sync obligingly writes
            # the requirement into both.
            if name in destinations:
                errors.append(
                    f"{capability}: ADDED '{name}' moves out of {origin}, but "
                    f"{destinations[name]} claims the same move -- one "
                    "requirement cannot arrive in two capabilities"
                )
                continue
            destinations[name] = capability
            if origin not in sources:
                sources[origin] = read_capability(live_specs, origin)
            leaving = sources[origin].get(name)
            if leaving is None:
                continue
            moved += 1
            before = normalized(leaving).splitlines()
            after = normalized(arriving).splitlines()
            if before == after:
                continue
            altered += 1
            if quiet:
                continue
            lines = list(
                difflib.unified_diff(
                    before,
                    after,
                    f"{origin} (live)",
                    f"{capability} (ADDED)",
                    n=1,
                    lineterm="",
                )
            )
            print(f"\n=== {change_id}: '{name}' moves {origin} -> {capability}")
            for line in lines[:MOVE_DIFF_LINES]:
                print(f"     {line}")
            if len(lines) > MOVE_DIFF_LINES:
                print(f"     ... {len(lines) - MOVE_DIFF_LINES} more diff lines")
    return errors, moved, altered


# Evidence lines shown for one state verdict. The partition of session-relay
# moved 94 requirements at once, and reporting every signal for a change that
# size buries the verdict under two hundred lines confirming that the parts
# which worked, worked.
EVIDENCE_LINES = 10


def evidence_for(label, evidence):
    """Return capped evidence lines, each prefixed with the change they concern."""
    lines = [f"{label}: {line}" for line in evidence[:EVIDENCE_LINES]]
    if len(evidence) > EVIDENCE_LINES:
        lines.append(
            f"{label}: ... {len(evidence) - EVIDENCE_LINES} further signals not shown"
        )
    return lines


def archived_dirs(arguments):
    """Return the unique archived change directories named by the arguments."""
    found = []
    for argument in arguments:
        path = argument if argument.startswith("/") else f"/{argument}"
        match = ARCHIVE_PATH.search(path)
        if match and match.group(1) not in found:
            found.append(match.group(1))
    return found


def being_archived(candidates, unstaged=False):
    """Narrow archived directories to the ones this commit is creating.

    The archive-readiness verdict is only true at the moment of archiving. It
    compares deltas against a live spec that later changes go on editing, so
    re-asking the question months afterwards gets a different and meaningless
    answer -- running it now against a change archived correctly this morning
    reports it unsynced, because a change that landed since rewrote two of the
    capabilities it touched.

    That matters because a commit amending an archived record touches the same
    paths as the commit that filed it. Without this, appending a note to an
    archived proposal would be gated on a verdict about that proposal's sync
    state, computed against a spec that has moved on -- and the only way past
    it would be bypassing the hook.

    The index distinguishes them: filing an archive adds or renames its files,
    while amending one modifies them. An index with nothing being ADDED is
    precisely the amendment case and returns nothing.

    A clean index means no commit is being made, so nothing is being filed and
    there is nothing to gate. Returning every candidate there was wrong in a
    way that surfaced only under `pre-commit run --all-files`, which hands the
    hook every archived change there has ever been: the gate then answered the
    retrospective question it exists to avoid, and failed with 461 errors about
    changes archived correctly months ago.

    `unstaged=True` is how a caller asks for that retrospective question on
    purpose -- replaying the gate over history one archive commit at a time,
    where the answer IS meaningful because the tree is the one that filed it.
    Nothing in the hook path passes it.
    """
    if unstaged:
        return candidates

    def staged(*filters):
        return subprocess.run(
            ["git", "diff", "--cached", "--name-only", *filters],
            capture_output=True,
            text=True,
            check=True,
        ).stdout

    try:
        additions = staged("--diff-filter=AR")
    except (OSError, subprocess.CalledProcessError):
        # Outside a repository there is no index to consult and no commit to
        # gate; say so by checking nothing rather than by checking everything.
        return []
    added = set(archived_dirs(additions.splitlines()))
    return [candidate for candidate in candidates if candidate in added]


def audit_archive(archived_dir, quiet):
    """Verify an archived change reached the live specs before it was filed.

    This is the gap the whole script grew out of. Syncing a change's deltas
    into the live specs is required by nothing: `opsx-archive` offers to sync
    and then archives whichever answer it gets, and once the deltas are under
    `changes/archive/` no validator reads them again, so a requirement that
    was specified and never synced simply never existed. The failure is silent
    at commit, silent at archive, and silent forever after.

    An archive commit is the last moment the two artifacts are still
    comparable, so it is the only place the check can be made.
    """
    home = planning_home()
    change_specs = home / "changes" / ARCHIVE_SEGMENT / archived_dir / "specs"
    if not change_specs.is_dir():
        # A change may legitimately carry no spec deltas at all.
        return []
    by_capability = read_change(change_specs)
    live_by_capability = {
        capability: read_capability(home / "specs", capability)
        for capability in by_capability
    }
    state, evidence = applied_state(by_capability, live_by_capability)
    if state in ("APPLIED", "SATISFIED"):
        # SATISFIED passes because nothing the deltas assert is missing from
        # live, which is the guarantee this gate exists to give. It is not the
        # same as knowing the change is what put it there, and it does not
        # catch a delta whose purpose was to DELETE an obligation; see
        # `applied_state` for why that case is not decidable here.
        if not quiet:
            print(
                f"\n{archived_dir}: deltas are present in the live specs"
                if state == "APPLIED"
                else f"\n{archived_dir}: every requirement, scenario and clause "
                "these deltas name is present in the live specs"
            )
        return []
    errors = [
        f"{archived_dir}: archiving deltas that have not reached the live "
        f"specs (state: {state}). Sync the change, then archive it -- once it "
        "is filed, nothing reads these deltas again and the requirements they "
        "carry will never have existed."
    ]
    errors.extend(evidence_for(archived_dir, evidence))
    return errors


def check_renames(capability, pairs, live, errors):
    """Validate RENAMED FROM/TO pairs; return the set of validated TO names.

    Only a name from a well-formed, individually-valid pair is returned. A
    malformed or partially invalid pair must not silently smuggle an unaudited
    MODIFIED requirement past the live-counterpart check.
    """
    validated = set()
    if len(pairs) % 2 != 0:
        errors.append(f"{capability}: RENAMED has an odd number of FROM/TO lines")
    for index in range(0, len(pairs) - 1, 2):
        (from_kind, from_name), (to_kind, to_name) = pairs[index], pairs[index + 1]
        if from_kind != "FROM" or to_kind != "TO":
            errors.append(
                f"{capability}: RENAMED pair at position {index // 2 + 1} is "
                f"not an alternating FROM/TO ({from_kind}, {to_kind})"
            )
            continue
        pair_valid = True
        if from_name not in live:
            errors.append(f"{capability}: RENAMED FROM '{from_name}' is not live")
            pair_valid = False
        if to_name in live:
            errors.append(f"{capability}: RENAMED TO '{to_name}' already exists live")
            pair_valid = False
        if pair_valid:
            validated.add(to_name)
    return validated


def audit(change_id, live_owners, inflight, quiet):
    """Audit one change's deltas. Returns (errors, dropped_scenarios, waits)."""
    resolved = resolve_paths(change_id)
    if resolved is None:
        return [f"no such change: '{change_id}'"], 0, []
    change_specs, live_specs = resolved
    if not change_specs.is_dir():
        return [f"{change_id}: no specs/ directory -- nothing to audit"], 0, []

    errors, drops, waits = [], 0, []
    by_capability = read_change(change_specs)
    live_by_capability = {
        capability: read_capability(live_specs, capability)
        for capability in by_capability
    }

    state, evidence = applied_state(by_capability, live_by_capability)
    if state == "MIXED":
        errors.append(
            f"{change_id}: deltas are half-synced -- some already applied to "
            "live and some not; sync the remainder or correct the delta"
        )
        errors.extend(evidence_for(change_id, evidence))
    if state == "APPLIED":
        # Every check below asks whether the delta agrees with the spec it will
        # modify. Once it has modified it, the question is answered and the
        # checks read backwards, so there is nothing left here to verify.
        print()
        print(
            f"{len(by_capability)} capabilities in '{change_id}' are already "
            "synced to live -- nothing to audit until the deltas change"
        )
        return errors, drops, waits

    for capability, (delta, pairs) in sorted(by_capability.items()):
        live = live_by_capability[capability]
        header_shown = False

        validated_renamed_to = check_renames(capability, pairs, live, errors)

        for operation in ("MODIFIED", "REMOVED"):
            for name in delta.get(operation, {}):
                # A MODIFIED requirement keyed by a RENAMED TO name has no
                # live counterpart under that name until sync rewrites the
                # live spec — its continuity is established by the validated
                # RENAMED pair above verifying the FROM name is live instead.
                if operation == "MODIFIED" and name in validated_renamed_to:
                    continue
                if name in live:
                    continue
                # A target that is missing because another change has not put
                # it there yet is an ordering constraint, not a defect. Failing
                # the commit instead is what made this delta unwritable until
                # its producer synced, while that producer was itself waiting
                # on this delta -- a deadlock with no exit but bypassing the
                # hook. Report the dependency and let the commit through.
                producers = [
                    (producer, target)
                    for producer, target in inflight.get(name, [])
                    if producer != change_id
                ]
                if producers:
                    waits.append((capability, operation, name, producers))
                    continue
                errors.append(
                    f"{capability}: {operation} '{name}' has no live counterpart"
                )

        for name in delta.get("ADDED", {}):
            if name in live:
                errors.append(
                    f"{capability}: ADDED '{name}' already exists live "
                    "(should this be MODIFIED?)"
                )

        for name, requirement in delta.get("MODIFIED", {}).items():
            before = live.get(name)
            if before is None:
                continue
            dropped = [s for s in before.scenarios if s not in requirement.scenarios]
            added = [s for s in requirement.scenarios if s not in before.scenarios]
            drops += len(dropped)
            if (dropped or added) and not quiet:
                if not header_shown:
                    print(f"\n=== {capability}")
                    header_shown = True
                print(f"\n  {name}")
                for scenario in dropped:
                    print(f"     - DROPPED  {scenario}")
                for scenario in added:
                    print(f"     + added    {scenario}")

    move_errors, moved, altered = check_moves(
        change_id, by_capability, live_specs, live_owners, quiet
    )
    errors.extend(move_errors)

    if waits and not quiet:
        print(f"\n=== {change_id} waits on another change's sync")
        for capability, operation, name, producers in waits:
            print(f"\n  {capability}: {operation} '{name}'")
            for producer, target in producers:
                print(f"     WAITS ON  {producer}, which puts it in {target}")

    print()
    print(f"{len(by_capability)} capabilities audited in '{change_id}'")
    if moved:
        print(
            f"{moved} requirement(s) relocated between capabilities, "
            f"{altered} of them altered in transit"
        )
    return errors, drops, waits


USAGE = (
    "usage: verify-openspec-deltas.py [--quiet] <change-id|delta-path> ...\n"
    "       verify-openspec-deltas.py [--quiet] --citations\n"
    "       verify-openspec-deltas.py [--quiet] --archived <archived-path> ...\n"
    "       verify-openspec-deltas.py --archived --unstaged <archived-path> ...\n"
    "\n"
    "  --unstaged  gate the named archives even though this commit is not\n"
    "              filing them; for replaying the check over history"
)

FLAGS = ("--quiet", "--citations", "--archived", "--unstaged")


def report(errors, trailer=None):
    """Print the error block and return the process exit status."""
    print()
    if errors:
        for message in errors:
            print(f"ERROR  {message}")
        print()
    print(f"{len(errors)} errors")
    if trailer:
        print(trailer)
    return 1 if errors else 0


def main():
    arguments = [a for a in sys.argv[1:] if a not in FLAGS]
    quiet = "--quiet" in sys.argv
    citations = "--citations" in sys.argv
    archived = "--archived" in sys.argv
    unstaged = "--unstaged" in sys.argv
    if not arguments and not citations:
        sys.exit(USAGE)

    failures = run_selftest()
    if failures:
        for failure in failures:
            print(f"verify-openspec-deltas: {failure}", file=sys.stderr)
        print(
            "verify-openspec-deltas: the applied-state classifier does not "
            "behave as specified, so it cannot be trusted to decide which "
            "checks apply",
            file=sys.stderr,
        )
        return 1

    if archived:
        # An archive commit is a directory move, so pre-commit may hand over a
        # proposal or a tasks file and no delta at all; the change is
        # identified by its directory either way. Nothing recognizable is not
        # a failure, for the same reason it is not in the per-change mode.
        errors, filing = [], being_archived(archived_dirs(arguments), unstaged)
        for archived_dir in filing:
            errors.extend(audit_archive(archived_dir, quiet))
        # Archiving is also the moment a citation waiting on this change's sync
        # stops being able to resolve, so the corpus check belongs on the same
        # commit rather than on some later one that happens to touch prose --
        # scoped to the changes being filed, which are the ones this commit is
        # answerable for.
        citation_errors, _ = check_citations(quiet=True, only_changes=filing)
        errors.extend(citation_errors)
        return report(errors)

    if citations:
        errors, pending = check_citations(quiet)
        trailer = None
        if pending:
            trailer = (
                f"\n{len(pending)} citation(s) resolve only into a change that has\n"
                "not synced yet. Each is a promise rather than a defect -- and\n"
                "each becomes permanent if that change archives unsynced."
            )
        return report(errors, trailer)

    # No recognizable change among the arguments is not a failure: the hook is
    # reached by any commit whose files match its filter, and an archive-only
    # commit legitimately names nothing to audit.
    selected = change_ids(arguments)
    if not selected:
        return 0

    live_owners, inflight, _ = name_universe(planning_home())

    errors, drops, waits = [], 0, 0
    for change_id in selected:
        change_errors, change_drops, change_waits = audit(
            change_id, live_owners, inflight, quiet
        )
        errors.extend(change_errors)
        drops += change_drops
        waits += len(change_waits)

    print()
    if errors:
        for message in errors:
            print(f"ERROR  {message}")
        print()
    print(f"{len(errors)} errors, {drops} dropped scenarios to confirm")
    if drops and not quiet:
        print(
            "\nConfirm each DROPPED line describes behavior this change retires.\n"
            "A MODIFIED delta replaces the whole requirement, so anything not\n"
            "carried forward is deleted from the live spec at sync time."
        )
    if waits and not quiet:
        print(
            f"\n{waits} delta target(s) are not live yet because another change\n"
            "has not synced. That is an ordering constraint, not a defect, but\n"
            "it is one somebody has to honor: this change cannot sync first."
        )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
