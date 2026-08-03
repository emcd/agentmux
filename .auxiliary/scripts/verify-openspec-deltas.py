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
`establish-delivery-commit-contract` dropped nine scenarios that survive the
change -- cursor-column gating, the operator-decision case, copy-mode delivery,
the activity-signal sources -- while rewriting the requirements around them.
Every one passed `--strict`. This script found them.

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

import json
import re
import subprocess
import sys
from pathlib import Path

REQUIREMENT = "### Requirement:"
SCENARIO = "#### Scenario:"


def resolve_paths(change_id):
    """Return (change_specs_dir, live_specs_dir) for a change.

    Resolved from `openspec status` rather than assumed, because the repository
    reaches its planning artifacts through an `openspec/` symlink and the real
    tree lives under `documentation/architecture/`. `changeRoot` is reported as
    the real path; `planningHome.changesDir` is reported through the symlink.
    Deriving both from `changeRoot` keeps them on the same side of it.
    """
    proc = subprocess.run(
        ["openspec", "status", "--change", change_id, "--json"],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.exit(f"openspec status failed for '{change_id}':\n{proc.stderr.strip()}")
    root = Path(json.loads(proc.stdout)["changeRoot"])
    return root / "specs", root.parent.parent / "specs"


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

    errors, drops, capabilities = [], 0, 0

    for delta_path in sorted(change_specs.glob("*/spec.md")):
        capability = delta_path.parent.name
        live = parse(live_specs / capability / "spec.md")
        delta = parse_delta(delta_path)
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
        capabilities += 1

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
