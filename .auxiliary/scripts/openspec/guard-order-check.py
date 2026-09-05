#!/usr/bin/env python3
''' Verify destructive verbs appear only in the probe library, behind a guard.

    Two rules, because the library and its callers make different promises.

    In `probe-worktree.sh`, a destructive verb is allowed but must run BEHIND a
    guard. Presence is not the property that matters: a wrapper that runs
    `git reset --hard` and *then* calls `probe::assert_inside` has already
    mutated the caller's tree before refusing. So this compares positions.

    In every other harness, a destructive verb is not allowed at all. That is
    the guarantee the README makes -- every destructive verb lives in the
    library -- and a check that audited only the library could not see it
    broken. It was broken exactly that way: a reviewer found `rm -rf` and an
    unguarded cleanup trap in the harness that runs this check.

    The mutants for the self-test are built in memory rather than written to
    disk, so this program creates and removes nothing.

    Usage: guard-order-check.py <probe-worktree.sh> [<harness.sh> ...]
'''

import pathlib
import re
import sys

DESTRUCTIVE = re.compile(
    r'git reset|git checkout --force|git clean|rm -rf|worktree remove')
GUARD = re.compile(r'probe::assert_inside|probe::_assert_disposable_path')
FUNCTION = re.compile(r'^(probe::[\w:]+)\(\) \{\n(.*?)^\}', re.S | re.M)

USAGE = 'usage: guard-order-check.py <probe-worktree.sh> [<harness.sh> ...]'


def uncommented(text):
    ''' Blank out comment lines, preserving offsets so positions stay valid. '''
    lines = []
    for line in text.splitlines():
        lines.append(' ' * len(line) if line.lstrip().startswith('#') else line)
    return '\n'.join(lines)


def audit_library(source):
    ''' Return (checked, failures): every verb must sit behind a guard. '''
    checked, failures = 0, []
    for name, body in FUNCTION.findall(source):
        code = uncommented(body)
        verb = DESTRUCTIVE.search(code)
        if verb is None:
            continue
        checked += 1
        guard = GUARD.search(code)
        if guard is None:
            failures.append(
                f'{name} runs {verb.group(0)!r} with no guard at all')
        elif guard.start() > verb.start():
            failures.append(
                f'{name} runs {verb.group(0)!r} BEFORE its guard '
                f'{guard.group(0)!r} -- the guard cannot prevent it')
    return checked, failures


def audit_harness(name, source):
    ''' Return failures: a harness may not spell a destructive verb at all. '''
    failures = []
    for number, line in enumerate(uncommented(source).splitlines(), start=1):
        found = DESTRUCTIVE.search(line)
        if found is not None:
            failures.append(
                f'{name}:{number} spells {found.group(0)!r} outside the '
                f'library -- route it through a guarded probe:: wrapper')
    return failures


# Each mutation breaks the library one way on purpose; both must be caught.
# `before` is asserted present so a restructured library reports staleness
# instead of silently mutating nothing.
MUTATIONS = (
    (
        'a wrapper whose guard was deleted',
        'probe::reset() {\n    probe::assert_inside\n',
        'probe::reset() {\n',
        'no guard at all',
    ),
    (
        'a wrapper whose guard runs after the verb',
        'probe::reset() {\n    probe::assert_inside\n    git reset --hard HEAD',
        'probe::reset() {\n    git reset --hard HEAD\n    probe::assert_inside',
        'BEFORE its guard',
    ),
)


def self_test(source):
    ''' Return failures: the library audit must reject each broken shape. '''
    failures = []
    for label, before, after, expected in MUTATIONS:
        if before not in source:
            failures.append(
                f'STALE: the library no longer has the shape the {label!r} '
                f'mutation edits, so it proves nothing')
            continue
        _, reported = audit_library(source.replace(before, after, 1))
        if not any(expected in failure for failure in reported):
            failures.append(
                f'the audit accepted {label} -- expected a report saying '
                f'{expected!r}, got {reported or "no failures"}')
    return failures


def main():
    if len(sys.argv) < 2:
        print(USAGE, file=sys.stderr)
        return 2
    library_path, harness_paths = sys.argv[1], sys.argv[2:]
    source = pathlib.Path(library_path).read_text()

    failures = self_test(source)
    for failure in failures:
        print(f'BAD   {failure}')
    if not failures:
        print(f'ok    the audit rejects all {len(MUTATIONS)} broken guard shapes')

    checked, library_failures = audit_library(source)
    failures.extend(library_failures)
    for failure in library_failures:
        print(f'BAD   {failure}')
    if not checked:
        print('BAD   found no destructive verbs -- the pattern is stale')
        failures.append('pattern stale')
    elif not library_failures:
        print(f'ok    all {checked} library wrappers guard before they act')

    harness_failures = []
    for path in harness_paths:
        harness_failures.extend(
            audit_harness(pathlib.Path(path).name,
                          pathlib.Path(path).read_text()))
    failures.extend(harness_failures)
    for failure in harness_failures:
        print(f'BAD   {failure}')
    if harness_paths and not harness_failures:
        print(f'ok    {len(harness_paths)} harnesses spell no destructive verb')

    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
