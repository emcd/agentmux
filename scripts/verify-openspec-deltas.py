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
from pathlib import Path

REQUIREMENT = "### Requirement:"
SCENARIO = "#### Scenario:"

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


def resolve_paths(change_id):
    """Return (change_specs_dir, live_specs_dir), or None if the change is absent."""
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
    change_root = home / "changes" / change_id
    if not change_root.is_dir():
        return None
    return change_root / "specs", home / "specs"


def parse(path):
    """Return {requirement_name: [scenario_name, ...]} for one spec file."""
    out, current = {}, None
    if not path.is_file():
        return out
    for line in path.read_text().splitlines():
        if line.startswith(REQUIREMENT):
            current = line[len(REQUIREMENT):].strip()
            out.setdefault(current, [])
        elif line.startswith(SCENARIO) and current is not None:
            out[current].append(line[len(SCENARIO):].strip())
    return out


def parse_delta(path):
    """Return {operation: {requirement_name: [scenario_name, ...]}}.

    Splits on `## <OP> Requirements` headers so each requirement is attributed
    to the operation it appears under.
    """
    blocks = re.split(r"^## ([A-Z]+) Requirements\s*$", path.read_text(), flags=re.M)
    result = {}
    for i in range(1, len(blocks), 2):
        operation, body = blocks[i], blocks[i + 1]
        section = result.setdefault(operation, {})
        current = None
        for line in body.splitlines():
            if line.startswith(REQUIREMENT):
                current = line[len(REQUIREMENT):].strip()
                section.setdefault(current, [])
            elif line.startswith(SCENARIO) and current is not None:
                section[current].append(line[len(SCENARIO):].strip())
        # A RENAMED block carries FROM:/TO: lines rather than requirement
        # headers; capture them as a pair list under a private key.
        if operation == "RENAMED":
            names = re.findall(
                r"^\s*-\s*(FROM|TO):\s*`?(?:### Requirement:)?\s*([^`\n]+?)`?\s*$",
                body,
                flags=re.M,
            )
            section["__pairs__"] = names
    return result


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


def audit(change_id, quiet):
    """Audit one change's deltas. Returns (errors, dropped_scenario_count)."""
    resolved = resolve_paths(change_id)
    if resolved is None:
        return [f"no such change: '{change_id}'"], 0
    change_specs, live_specs = resolved
    if not change_specs.is_dir():
        return [f"{change_id}: no specs/ directory -- nothing to audit"], 0

    errors, drops = [], 0

    # OpenSpec declares the specs artifact as `specs/**/*.md`, so a capability
    # may be split across several files rather than a single `spec.md`. Group
    # every file under its capability directory and merge their deltas before
    # comparing, or a requirement defined in a sibling file reads as missing.
    by_capability = {}
    for delta_path in sorted(change_specs.rglob("*.md")):
        capability = delta_path.relative_to(change_specs).parts[0]
        by_capability.setdefault(capability, []).append(delta_path)

    for capability, delta_paths in sorted(by_capability.items()):
        # Merge every live file for the capability, skipping only the root
        # spec.md already parsed. Filtering by basename instead would skip a
        # nested `<capability>/parts/spec.md` too, and every requirement defined
        # there would be falsely reported as having no live counterpart.
        root_spec = live_specs / capability / "spec.md"
        live = parse(root_spec)
        for extra in sorted((live_specs / capability).rglob("*.md")):
            if extra != root_spec:
                for name, scenarios in parse(extra).items():
                    live.setdefault(name, []).extend(scenarios)

        delta = {}
        for delta_path in delta_paths:
            for operation, requirements in parse_delta(delta_path).items():
                section = delta.setdefault(operation, {})
                for name, scenarios in requirements.items():
                    section.setdefault(name, []).extend(scenarios)
        header_shown = False

        for operation in ("MODIFIED", "REMOVED"):
            for name in delta.get(operation, {}):
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

        pairs = delta.get("RENAMED", {}).get("__pairs__", [])
        for kind, name in pairs:
            if kind == "FROM" and name not in live:
                errors.append(f"{capability}: RENAMED FROM '{name}' is not live")
            if kind == "TO" and name in live:
                errors.append(f"{capability}: RENAMED TO '{name}' already exists live")

        for name, scenarios in delta.get("MODIFIED", {}).items():
            before = live.get(name)
            if before is None:
                continue
            dropped = [s for s in before if s not in scenarios]
            added = [s for s in scenarios if s not in before]
            drops += len(dropped)
            if (dropped or added) and not quiet:
                if not header_shown:
                    print(f"\n=== {capability}")
                    header_shown = True
                print(f"\n  {name}")
                for s in dropped:
                    print(f"     - DROPPED  {s}")
                for s in added:
                    print(f"     + added    {s}")

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
