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

Four checks, three of which are errors and one of which is a report:

- ERROR   MODIFIED or REMOVED naming a requirement that does not exist live
          (a typo, or a rename that happened upstream)
- ERROR   ADDED naming a requirement that already exists live
          (it should be MODIFIED, or the delta will collide at sync)
- ERROR   RENAMED whose FROM is absent live, or whose TO already exists
- REPORT  scenarios present live but absent from a MODIFIED delta

The last is a report rather than an error because dropping a scenario is often
exactly right -- it describes retired behavior. The point is that each drop
should be a decision someone made, not something that happened. Read the list
and confirm every line. Only an ERROR sets a non-zero exit status.

Usage:  scripts/verify-openspec-deltas.py <change-id> [<change-id> ...]
        scripts/verify-openspec-deltas.py [--quiet] <delta-spec-path> ...

Arguments are change ids, or paths to delta spec files from which the change id
is read. The path form is what the `lint-openspec-deltas` pre-commit hook uses:
pre-commit hands it the staged delta specs, and each change they belong to is
audited once. Paths naming an archived change are skipped -- an archived change
is a completed record rather than a delta under authorship.

Run from anywhere; paths resolve from this script's own location. Run it after
`openspec validate <change-id> --strict` passes and before requesting review.

The pre-commit hook covers every commit that touches a delta spec. It cannot
cover the archive-order case: when another change archives into the same
requirement, the live spec moves beneath an already-committed delta and the drop
set changes with no delta file touched, so no commit fires the hook. Run this by
hand against the post-archive live spec in that case.
"""

import re
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

# An archived change lives under `changes/archive/<date>-<id>/`, so its id
# segment reads as the literal `archive`.
ARCHIVE_SEGMENT = "archive"


def planning_home():
    """Return the OpenSpec planning home directory, or exit if there is none."""
    repo_root = Path(__file__).resolve().parents[1]
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


def audit(change_id, quiet):
    """Audit one change's deltas. Returns (errors, dropped_scenario_count)."""
    resolved = resolve_paths(change_id)
    if resolved is None:
        return [f"no such change: '{change_id}'"], 0
    change_specs, live_specs = resolved
    if not change_specs.is_dir():
        return [f"{change_id}: no specs/ directory -- nothing to audit"], 0

    errors, drops = [], 0
    by_capability = read_change(change_specs)

    for capability, (delta, pairs) in sorted(by_capability.items()):
        live = read_capability(live_specs, capability)
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
                if name not in live:
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

    print()
    print(f"{len(by_capability)} capabilities audited in '{change_id}'")
    return errors, drops


USAGE = "usage: verify-openspec-deltas.py [--quiet] <change-id|delta-path> ..."


def main():
    arguments = [a for a in sys.argv[1:] if a != "--quiet"]
    quiet = "--quiet" in sys.argv
    if not arguments:
        sys.exit(USAGE)

    # No recognizable change among the arguments is not a failure: the hook is
    # reached by any commit whose files match its filter, and an archive-only
    # commit legitimately names nothing to audit.
    selected = change_ids(arguments)
    if not selected:
        return 0

    errors, drops = [], 0
    for change_id in selected:
        change_errors, change_drops = audit(change_id, quiet)
        errors.extend(change_errors)
        drops += change_drops

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
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
