#!/usr/bin/env python3
''' Confirm the script's self-tests actually fail when the checkers break.

    A self-test that passes against a broken checker is worse than none, since
    it certifies the thing it cannot see. Each mutation below breaks one
    decision on purpose; every one must be CAUGHT.

    This harness is deliberately coupled to exact source strings in
    `scripts/verify-openspec-deltas.py`, and is EXPECTED to need updating
    whenever that file changes. A `STALE` line means the mutated text has moved
    and the mutation needs rewriting -- it does not mean the harness can be
    ignored, and a stale mutation proves nothing about the decision it was
    written to defend. That loudness is the reason it is versioned rather than
    left as a scratch file: it reports its own rot instead of passing quietly.

    Writes nothing outside the repository, and refuses to start if its one
    scratch file already exists.
'''

import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
SCRIPT = ROOT / 'scripts' / 'verify-openspec-deltas.py'
# The mutant must sit exactly one directory below the repository root: the
# script derives the repository root as `Path(__file__).resolve().parents[1]`,
# so a mutant any deeper resolves its planning home outside the repository and
# every corpus-dependent mutation goes quiet instead of failing.
SCRATCH = ROOT / '.auxiliary' / 'mutant-verify-openspec-deltas.py'

# `(label, before, after, mode, expected_marker)`
MUTATIONS = [
    ('applied: half-synced reads as synced',
     'return "MIXED", pending + applied', 'return "APPLIED", []',
     '--citations', 'applied-state case'),
    ('applied: synced names read as pending',
     '    if applied:\n        return "APPLIED", []',
     '    if applied:\n        return "PENDING", []',
     '--citations', 'applied-state case'),
    ('applied: pending names read as synced',
     '    if pending:\n        return "PENDING", pending',
     '    if pending:\n        return "APPLIED", []',
     '--citations', 'applied-state case'),
    ('applied: MODIFIED evidence discarded',
     '            if introduced or rewritten:',
     '            if False:',
     '--citations', 'applied-state case'),
    ('applied: normative-clause evidence discarded',
     '                if clause not in live_text',
     '                if False',
     '--citations', 'applied-state case'),
    ('applied: clause comparison not markup-insensitive',
     'return " ".join(re.sub(r"[*`_]+", "", text).split())',
     'return text',
     '--citations', 'applied-state case'),
    ('applied: list lines not excluded from both sides',
     '        if not line.lstrip().startswith(("-", "*", "+"))',
     '        if True',
     '--citations', 'applied-state case'),
    ('applied: RENAMED evidence discarded',
     '        for from_name, to_name in rename_pairs(pairs):',
     '        for from_name, to_name in []:',
     '--citations', 'applied-state case'),
    # Removing any single branch of the rename chain still lands on a pending
    # arm, so the mutation that matters is one that lets a shape other than the
    # completed one count as SYNCED.
    ('applied: a rename with the old name still live counts as synced',
     '            if from_name not in live and to_name in live:',
     '            if from_name in live or to_name in live:',
     '--citations', 'applied-state case'),
    ('applied: a rename leaving neither name live reads as no evidence',
     '            else:\n                pending.append(\n                    f"unsynced: {capability}: RENAMED',
     '            elif False:\n                pending.append(\n                    f"unsynced: {capability}: RENAMED',
     '--citations', 'applied-state case'),
    ('applied: no unmet content folded into PENDING',
     '    return "SATISFIED", []',
     '    return "PENDING", []',
     '--citations', 'applied-state case'),
    ('move: two destinations for one source permitted',
     '            if name in destinations:',
     '            if False:',
     '--citations', 'move case'),
    ('move: cross-capability duplicate permitted',
     '            if origin not in removed_from.get(name, []):',
     '            if False:',
     '--citations', 'move case'),
    ('citation: dangling branch removed',
     '        return None if cites_its_archive(document, sources) else ("DANGLING", sources)',
     '        return None',
     '--citations', 'citation case'),
    ('citation: archive-naming exemption never fires',
     '        return None if cites_its_archive(document, sources) else ("DANGLING", sources)',
     '        return ("DANGLING", sources)',
     '--citations', 'citation case'),
    ('citation: archive naming matched by unbounded substring',
     'if re.search(rf"(?<![\\w-]){re.escape(candidate)}(?![\\w-])", document):',
     'if candidate in document:',
     '--citations', 'citation case'),
    # The wiring mutation cannot be caught by a self-test: the cases call
    # `classify_citation` directly, so dropping the argument at the one call
    # site leaves them passing. It was originally caught by the live corpus --
    # a design note naming its own archive would start failing again -- which
    # made its fixture whichever document happened to carry that shape. That
    # fixture evaporated the moment the change carrying the note was archived,
    # and the mutation went quiet exactly when the corpus it depended on moved.
    #
    # So the wiring is no longer defended by a fixture at all: `document` is a
    # required parameter, and dropping it is a TypeError rather than a silent
    # behavior change. The mutation now asserts that crash, which depends on
    # nothing outside the two files involved.
    ('citation: citing document not consulted',
     'verdict = classify_citation(name, owner, live, inflight, archived, text)',
     'verdict = classify_citation(name, owner, live, inflight, archived)',
     '--citations', "missing 1 required positional argument: 'document'"),
    ('citation: pending branch never fires',
     '        return None if owner in sources else ("PENDING", sources)',
     '        return None',
     '--citations', 'citation case'),
    ('citation: self-citation not exempted',
     '        return None if owner in sources else ("PENDING", sources)',
     '        return ("PENDING", sources)',
     '--citations', 'citation case'),
    ('citation: live names not treated as resolved',
     '    if name in live:\n        return None\n    if name in inflight:',
     '    if name in inflight:',
     '--citations', 'citation case'),
]


def main():
    # The mutant is a deliberately broken copy of a checker this repository
    # gates commits with, so it must never overwrite something already at that
    # path -- refuse rather than clobber, the same posture the shell harnesses
    # take toward the working tree.
    if SCRATCH.exists():
        print(f'refusing to overwrite {SCRATCH}', file=sys.stderr)
        return 1
    try:
        return run_mutations()
    finally:
        # The mutant sits in a tracked directory, so an abandoned copy would
        # show up in `git status` rather than staying invisible.
        SCRATCH.unlink(missing_ok=True)


def run_mutations():
    source = SCRIPT.read_text()
    SCRATCH.parent.mkdir(parents=True, exist_ok=True)
    missed = 0
    for label, before, after, mode, marker in MUTATIONS:
        if before not in source:
            print(f'STALE   {label} -- the mutated text is no longer present')
            missed += 1
            continue
        SCRATCH.write_text(source.replace(before, after, 1))
        result = subprocess.run(
            [sys.executable, str(SCRATCH), mode],
            capture_output=True, text=True, cwd=ROOT)
        # Self-test failures are reported on stderr; corpus findings on stdout.
        # A marker is distinctive enough to search both without a self-test
        # marker ever matching ordinary output.
        caught = result.returncode == 1 and marker in (result.stderr + result.stdout)
        print(f"{'CAUGHT ' if caught else 'MISSED '} {label}")
        missed += 0 if caught else 1
    print(f'\n{len(MUTATIONS) - missed}/{len(MUTATIONS)} mutations caught')
    return 1 if missed else 0


if __name__ == '__main__':
    sys.exit(main())
