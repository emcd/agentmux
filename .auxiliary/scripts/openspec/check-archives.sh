#!/usr/bin/env bash
# Run the archive-readiness gate at the commit that filed each archive.
#
#   check-archives.sh [count]     sweep that many recent archives; default 25
#   check-archives.sh <dir-name>  check one archived change by directory name
#
# Set VERBOSE=1 to print each flagged change's errors.
#
# The gate's verdict is only meaningful at the moment of archiving: it compares
# deltas against a live spec that later changes go on editing. So it cannot be
# evaluated against today's tree -- it has to run at each historical archive
# commit, with today's checker copied in. That is what the probe worktree is
# for.
#
# Also the tool for "was requirement X ever synced?" -- name the archived change
# that introduced it and read the verdict.
#
# Run from anywhere in the repository. Every mutation happens inside a throwaway
# worktree under `.auxiliary/temporary/`; see probe-worktree.sh, which owns the
# guard that enforces it.
set -euo pipefail

source "$(dirname "$0")/probe-worktree.sh"

# A count sweeps that many recent archive commits; a name checks just that one.
COUNT=25
ONLY=""
case "${1:-}" in
    "") ;;
    *[!0-9]*) ONLY="$1" ;;
    *) COUNT="$1" ;;
esac
ARCHIVE='documentation/architecture/openspec/changes/archive'

# Enumerate the archive-filing commits before entering the probe, so the list
# comes from the branch the caller is actually on.
#
# `git log -n` rather than a pipe to `head`: under `pipefail` the SIGPIPE from
# an early-closing `head` fails the assignment, and `set -e` then exits the
# sweep silently before it checks anything.
SEARCH="$COUNT"
[ -n "$ONLY" ] && SEARCH=200
COMMITS=$(git log --diff-filter=A --format='%h' -n "$SEARCH" \
    -- "$ARCHIVE/*/proposal.md")

probe::enter gate-probe

flagged=0
checked=0
satisfied=0
for commit in $COMMITS; do
    dir=$(git show --name-only --format='' "$commit" -- "$ARCHIVE/*/proposal.md" \
        | head -1 | cut -d/ -f6)
    [ -n "$dir" ] || continue
    if [ -n "$ONLY" ] && [ "$dir" != "$ONLY" ]; then continue; fi
    probe::checkout "$commit"
    probe::install_checker
    checked=$((checked + 1))
    full=$(probe::audit --archived --unstaged "$ARCHIVE/$dir/proposal.md")
    result=$(printf '%s' "$full" | tail -1)
    # A pass is not one thing: the gate can confirm the deltas are live, or
    # admit it cannot tell. The second is the one worth counting, because it is
    # the only shape in which an unsynced change still gets through.
    case "$full" in
        *"is present in the live specs"*) satisfied=$((satisfied + 1)) ;;
    esac
    if [ "$result" != "0 errors" ]; then
        flagged=$((flagged + 1))
        echo "  FLAGGED  $dir -> $result"
        if [ "${VERBOSE:-}" = "1" ]; then
            printf '%s\n' "$full" | grep '^ERROR' | sed 's/^/       /' || true
        fi
    fi
done
echo "$flagged flagged, $satisfied passed on content-present, of $checked archive commits checked"
