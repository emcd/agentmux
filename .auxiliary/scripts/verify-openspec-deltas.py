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
and confirm every line.

Usage:  .auxiliary/scripts/verify-openspec-deltas.py <change-id>
        .auxiliary/scripts/verify-openspec-deltas.py --quiet <change-id>

Run from the repository root, after `openspec validate <change-id> --strict`
passes and before requesting review. Exits non-zero on any ERROR.
"""

import re
import sys
from pathlib import Path

REQUIREMENT = "### Requirement:"
SCENARIO = "#### Scenario:"

# The planning home, relative to the repository root. The repository also
# carries an `openspec/` symlink to this directory, but that symlink is
# untracked and listed in `.git/info/exclude`, so it exists only in worktrees
# where somebody created it by hand. Shelling out to `openspec status` to
# resolve these paths therefore fails in a fresh clone. Deriving them from this
# script's own location instead is deterministic everywhere.
PLANNING_HOME = Path("documentation/architecture/openspec")


def resolve_paths(change_id):
    """Return (change_specs_dir, live_specs_dir) for a change."""
    repo_root = Path(__file__).resolve().parents[2]
    home = repo_root / PLANNING_HOME
    if not home.is_dir():
        # Fall back to the symlink for a layout this constant has outlived.
        home = (repo_root / "openspec").resolve()
    if not home.is_dir():
        sys.exit(
            f"cannot locate the OpenSpec planning home; looked for "
            f"{repo_root / PLANNING_HOME} and {repo_root / 'openspec'}"
        )
    change_root = home / "changes" / change_id
    if not change_root.is_dir():
        sys.exit(f"no such change: '{change_id}' (looked in {home / 'changes'})")
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


USAGE = "usage: verify-openspec-deltas.py [--quiet] <change-id>"


def main():
    argv = [a for a in sys.argv[1:] if a != "--quiet"]
    quiet = "--quiet" in sys.argv
    if len(argv) != 1:
        sys.exit(USAGE)

    change_specs, live_specs = resolve_paths(argv[0])
    if not change_specs.is_dir():
        sys.exit(f"no specs/ directory in change '{argv[0]}' -- nothing to audit")

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
        live = parse(live_specs / capability / "spec.md")
        for extra in sorted((live_specs / capability).glob("**/*.md")):
            if extra.name != "spec.md":
                for name, scenarios in parse(extra).items():
                    live.setdefault(name, []).extend(scenarios)

        delta = {}
        for delta_path in delta_paths:
            for operation, requirements in parse_delta(delta_path).items():
                section = delta.setdefault(operation, {})
                for name, scenarios in requirements.items():
                    if name == "__pairs__":
                        section.setdefault(name, []).extend(scenarios)
                    else:
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

    capabilities = len(by_capability)
    print()
    if errors:
        for message in errors:
            print(f"ERROR  {message}")
        print()
    print(
        f"{capabilities} capabilities audited, {len(errors)} errors, "
        f"{drops} dropped scenarios to confirm"
    )
    if drops and not quiet:
        print(
            "\nConfirm each DROPPED line describes behavior this change retires.\n"
            "A MODIFIED delta replaces the whole requirement, so anything not\n"
            "carried forward is deleted from the live spec at sync time."
        )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
