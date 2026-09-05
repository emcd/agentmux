#!/usr/bin/env bash
# Prove the probe-worktree guard refuses, and that nothing bypasses it.
#
# The other harnesses are safe to hand to a reviewer only because every
# destructive verb they reach goes through probe-worktree.sh and refuses to run
# outside a throwaway worktree. That claim is worth exactly as much as the
# evidence for it, so this exercises the refusal rather than asserting it.
#
# Note what is deliberately NOT tested: calling `probe::reset` from the real
# checkout to watch it refuse. If the guard were broken that test would destroy
# the caller's uncommitted work, which is the outcome the guard exists to
# prevent. `probe::assert_inside` is tested directly instead -- it only refuses,
# never mutates -- and `guard-order-check.py` establishes that every destructive
# wrapper calls a guard BEFORE the verb it protects. That ordering requirement
# is not pedantry: a reviewer pointed out that an earlier presence-only check
# would have passed a wrapper which resets first and refuses afterwards, and
# fixing it found exactly that shape in this library's own discard path.
#
# Run from anywhere in the repository. Writes nothing outside
# `.auxiliary/temporary/`.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LIBRARY="$HERE/probe-worktree.sh"
failures=0

report() {  # report <ok|BAD> <description>
    printf '%-5s %s\n' "$1" "$2"
    [ "$1" = "ok" ] || failures=$((failures + 1))
}

expect_refusal() {  # expect_refusal <description> <expected-fragment> <script>
    local description="$1" fragment="$2" script="$3" output status
    output=$(bash -c "$script" 2>&1)
    status=$?
    if [ "$status" -eq 0 ]; then
        report BAD "$description -- exited 0 instead of refusing"
    elif [[ "$output" != *"$fragment"* ]]; then
        report BAD "$description -- refused with unexpected message: $output"
    else
        report ok "$description"
    fi
}

expect_refusal 'refuses before probe::enter has run' \
    'probe::enter has not run' \
    "source '$LIBRARY'; probe::assert_inside"

expect_refusal 'refuses once the working directory leaves the worktree' \
    'refusing to mutate outside the probe worktree' \
    "source '$LIBRARY'; probe::enter guard-probe; cd \"\$PROBE_REPO_ROOT\"; probe::assert_inside"

expect_refusal 'refuses a removal path outside .auxiliary/temporary' \
    'refusing to remove outside' \
    "source '$LIBRARY'; PROBE_REPO_ROOT=/tmp/x; probe::_assert_disposable_path /tmp/x/src"

expect_refusal 'refuses a nested removal path' \
    'refusing to remove a nested path' \
    "source '$LIBRARY'; PROBE_REPO_ROOT=/tmp/x; probe::_assert_disposable_path /tmp/x/.auxiliary/temporary/a/b"

expect_refusal 'refuses a probe name that is not one path segment' \
    'must be a single path segment' \
    "source '$LIBRARY'; probe::enter ../escape"

# The positive case: inside its own worktree the guard permits, and the
# worktree is genuinely separate from the caller's.
output=$(bash -c "source '$LIBRARY'; probe::enter guard-probe; probe::assert_inside && echo INSIDE:\$PWD" 2>&1)
if [[ "$output" == *"INSIDE:"*"/.auxiliary/temporary/guard-probe"* ]]; then
    report ok 'permits inside the worktree it created'
else
    report BAD "permits inside the worktree it created -- got: $output"
fi

# The worktree must not survive the script that made it.
output=$(bash -c "source '$LIBRARY'; probe::enter guard-probe" 2>&1)
if [ -e "$(git rev-parse --show-toplevel)/.auxiliary/temporary/guard-probe" ]; then
    report BAD 'removes the worktree on exit -- it is still there'
else
    report ok 'removes the worktree on exit'
fi

# Static check: every destructive verb must run BEHIND a guard, not merely in a
# function that mentions one somewhere. A wrapper that resets and then asserts
# has already mutated the caller's tree before refusing. The same program also
# audits the harnesses, which may spell no destructive verb at all, and
# self-tests its own rejection of both broken shapes using in-memory mutants --
# so nothing here creates or removes a file.
if python3 "$HERE/guard-order-check.py" "$LIBRARY" \
        "$HERE/check-archives.sh" "$HERE/check-archive-scope.sh" \
        "$HERE/check-probes.sh" "$HERE/check-probe-guard.sh"; then
    :
else
    failures=$((failures + 1))
fi

echo
echo "$failures guard problems"
[ "$failures" -eq 0 ]
