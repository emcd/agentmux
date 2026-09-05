#!/usr/bin/env bash
# Confirm the archive gate fires on filing an archive and not on amending one.
#
# The gate's verdict is only true at the moment of archiving, so it must not be
# re-asked later. A commit that appends a note to an archived proposal touches
# the same paths as the commit that filed it, and the index is what tells them
# apart: filing adds or renames, amending modifies.
#
# The later cases cover the shapes the gate used to pass: a MODIFIED delta that
# rewrites a SHALL while leaving every scenario heading alone, and a rename
# whose two halves cancel to no-evidence.
#
# Run from anywhere in the repository. All staging happens inside a throwaway
# worktree under `.auxiliary/temporary/`, so the caller's index and working tree
# are never touched. See probe-worktree.sh for the guard that enforces it.
set -euo pipefail

source "$(dirname "$0")/probe-worktree.sh"
probe::enter scope-probe
probe::install_checker

ARCHIVE='documentation/architecture/openspec/changes/archive'
SPECS='documentation/architecture/openspec/specs'

# Pick any archived change; the point is the index status, not the verdict.
DIR=$(ls "$ARCHIVE" | tail -1)
TARGET="$ARCHIVE/$DIR/proposal.md"
CAP=$(ls "$SPECS" | head -1)
LIVE="$SPECS/$CAP/spec.md"
echo "using archived change: $DIR"
echo "using live capability: $CAP"

# Each case starts from a clean worktree so no case inherits another's index.
# `probe::reset` is guarded and cannot reach outside the probe; it also drops
# the installed checker, hence the reinstall.
case_setup() {
    probe::reset
    probe::install_checker
    echo
    echo "########## $1"
}

case_setup "amending an archived record (index status M)"
printf '\n<!-- probe note -->\n' >> "$TARGET"
git add "$TARGET"
git diff --cached --name-status | sed 's/^/  index: /'
probe::audit --archived "$TARGET" | tail -3
echo "  expected: 0 errors, because this commit is not filing an archive"

case_setup "filing an archive (index status A)"
NEW="$ARCHIVE/2026-01-01-zz-probe-never-synced/specs/authorization-scope"
mkdir -p "$NEW"
cat > "$NEW/spec.md" <<'DELTA'
## ADDED Requirements

### Requirement: A Requirement That Never Reached Live

The relay SHALL do something no live spec records.

#### Scenario: Never synced

- **WHEN** this change is archived without syncing
- **THEN** the requirement it carries has never existed
DELTA
git add "$NEW/spec.md"
git diff --cached --name-status | sed 's/^/  index: /'
probe::audit --archived "$NEW/spec.md" | tail -4
echo "  expected: an error, because this commit IS filing an unsynced archive"

case_setup "filing an unsynced NORMATIVE PROSE edit (no new scenario)"
# Reuse a heading the live requirement already has, so the ONLY difference is
# the normative clause. Otherwise the scenario signal fires and the clause
# check is not the thing being tested.
NAME=$(grep -m1 '^### Requirement: ' "$LIVE" | sed 's/^### Requirement: //')
SCENARIO=$(grep -m1 '^#### Scenario: ' "$LIVE" | sed 's/^#### Scenario: //')
PROSE="$ARCHIVE/2026-01-01-zz-probe-prose-only/specs/$CAP"
mkdir -p "$PROSE"
{
    echo '## MODIFIED Requirements'
    echo
    echo "### Requirement: $NAME"
    echo
    echo 'The relay SHALL observe a brand new obligation that no live spec states.'
    echo
    echo "#### Scenario: $SCENARIO"
    echo
    echo '- **WHEN** something happens'
    echo '- **THEN** something follows'
} > "$PROSE/spec.md"
git add "$PROSE/spec.md"
echo "  requirement: $NAME"
probe::audit --archived "$PROSE/spec.md" | tail -3
echo "  expected: an error naming an unsynced normative clause"

case_setup "filing an unsynced RENAME whose new name already exists"
# FROM is still live, so the rename never removed the old name, but TO also
# exists for an unrelated reason. The two signals used to cancel to
# no-evidence. Both names are taken from the live spec, so this is that state.
FROM_NAME=$(grep -m1 '^### Requirement: ' "$LIVE" | sed 's/^### Requirement: //')
TO_NAME=$(grep '^### Requirement: ' "$LIVE" | sed -n '2p' | sed 's/^### Requirement: //')
REN="$ARCHIVE/2026-01-01-zz-probe-rename/specs/$CAP"
mkdir -p "$REN"
{
    echo '## RENAMED Requirements'
    echo
    echo "- FROM: \`### Requirement: $FROM_NAME\`"
    echo "- TO: \`### Requirement: $TO_NAME\`"
} > "$REN/spec.md"
git add "$REN/spec.md"
echo "  FROM: $FROM_NAME"
echo "  TO:   $TO_NAME   (both already live)"
probe::audit --archived "$REN/spec.md" | tail -3
echo "  expected: an error saying the rename left both names live"
