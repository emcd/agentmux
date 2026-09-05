#!/usr/bin/env bash
# Guarded throwaway-worktree helpers for the OpenSpec audit harnesses.
#
# Every destructive verb these harnesses use lives in this file, and every one
# of them refuses to run unless the working directory is inside a worktree this
# library created under `.auxiliary/temporary/`. That is the whole point: a
# reader confirms the safety property once, here, instead of tracing each
# caller's control flow to satisfy themselves that a `git reset --hard` cannot
# reach their own uncommitted work.
#
# The harnesses therefore never spell a destructive verb themselves. If you are
# adding one and reach for `git reset`, `git checkout --force`, or `rm -rf`,
# add a guarded wrapper here instead.
#
# Source this file, then call `probe::enter <name>` before anything else:
#
#   source "$(dirname "$0")/probe-worktree.sh"
#   probe::enter gate-probe
#
# After that call the working directory IS the throwaway worktree, and
# `$PROBE_REPO_ROOT` is the real checkout for reading files out of.

# Absolute path of the real checkout, set by `probe::enter`.
PROBE_REPO_ROOT=''
# Absolute path of the throwaway worktree, set by `probe::enter`.
PROBE_DIR=''

probe::_fail() {
    printf 'probe-worktree: %s\n' "$1" >&2
    exit 1
}

# Create a detached throwaway worktree and make it the working directory.
probe::enter() {
    local name="$1"
    [ -n "$name" ] || probe::_fail 'probe::enter needs a name'
    case "$name" in
        */*|.|..) probe::_fail "probe name must be a single path segment: $name" ;;
    esac

    PROBE_REPO_ROOT=$(git rev-parse --show-toplevel) \
        || probe::_fail 'not inside a git repository'
    PROBE_DIR="$PROBE_REPO_ROOT/.auxiliary/temporary/$name"

    # `.auxiliary/temporary` is git-ignored, so the worktree never appears as
    # untracked noise in the caller's status.
    probe::_discard
    trap probe::_discard EXIT
    git worktree add --detach "$PROBE_DIR" HEAD >/dev/null 2>&1 \
        || probe::_fail "could not create worktree at $PROBE_DIR"
    cd "$PROBE_DIR" || probe::_fail "could not enter $PROBE_DIR"
    # Prove the guard holds before any caller relies on it.
    probe::assert_inside
}

probe::_discard() {
    [ -n "$PROBE_DIR" ] || return 0
    # This is the one path that cannot use `probe::assert_inside` -- it runs
    # after the working directory has left the worktree, and on paths where the
    # worktree may never have existed -- so it checks the path instead, before
    # either destructive verb. `git worktree remove --force` destroys a
    # registered worktree, so it must be behind the assertion too, not only the
    # `rm -rf` that follows it.
    probe::_assert_disposable_path "$PROBE_DIR"
    cd "$PROBE_REPO_ROOT" 2>/dev/null || true
    git worktree remove --force "$PROBE_DIR" >/dev/null 2>&1 || true
    # A worktree whose creation half-failed leaves a directory `remove` will not
    # claim.
    rm -rf "$PROBE_DIR"
}

# Refuse a path that is not a named directory directly under the repository's
# git-ignored `.auxiliary/temporary/`.
probe::_assert_disposable_path() {
    case "$1" in
        "$PROBE_REPO_ROOT"/.auxiliary/temporary/*/*)
            probe::_fail "refusing to remove a nested path: $1" ;;
        "$PROBE_REPO_ROOT"/.auxiliary/temporary/?*)
            : ;;
        *)
            probe::_fail "refusing to remove outside .auxiliary/temporary: $1" ;;
    esac
}

# Refuse unless the working directory is the throwaway worktree.
#
# Checked against git's own idea of the enclosing worktree rather than against
# `$PWD`, so a caller that wandered into a subdirectory still passes and one
# that wandered into the real checkout does not.
probe::assert_inside() {
    [ -n "$PROBE_DIR" ] || probe::_fail 'probe::enter has not run'
    local here
    here=$(git rev-parse --show-toplevel 2>/dev/null) \
        || probe::_fail 'not inside a git repository'
    [ "$here" = "$PROBE_DIR" ] \
        || probe::_fail "refusing to mutate outside the probe worktree: $here"
}

# Discard everything in the probe worktree, back to its checked-out commit.
probe::reset() {
    probe::assert_inside
    git reset --hard HEAD >/dev/null
    git clean -fd >/dev/null
}

# Move the probe worktree to a historical commit, discarding what it holds.
probe::checkout() {
    probe::assert_inside
    git checkout --force --detach "$1" >/dev/null 2>&1 \
        || probe::_fail "could not check out $1"
}

# Copy today's checker into the probe worktree.
#
# The script derives the repository root from its own location, so the copy
# audits the probe worktree's planning home rather than the real one. Early
# commits predate `scripts/` entirely, hence the mkdir.
probe::install_checker() {
    probe::assert_inside
    mkdir -p scripts
    cp "$PROBE_REPO_ROOT/scripts/verify-openspec-deltas.py" \
        scripts/verify-openspec-deltas.py
}

# Run the checker in the probe worktree, letting a non-zero exit through.
#
# A finding is the result these harnesses are looking for, not a fault, so
# `set -e` must not treat it as one.
probe::audit() {
    probe::assert_inside
    python3 scripts/verify-openspec-deltas.py "$@" 2>&1 || true
}
