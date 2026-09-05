#!/usr/bin/env bash
# Exercise the audit branches that no real change currently reaches.
#
# Several of the checker's decisions have no instance in today's corpus, so the
# only evidence they work is a constructed one. Each probe builds a throwaway
# change and runs the audit against it.
#
# Run from anywhere in the repository. Every file this writes lands inside a
# throwaway worktree under `.auxiliary/temporary/`, so the real planning home is
# never touched and a crash mid-run leaves nothing behind for `openspec
# validate` to trip over. See probe-worktree.sh for the guard.
set -euo pipefail

source "$(dirname "$0")/probe-worktree.sh"
probe::enter probes
# The worktree is checked out at HEAD, so without this the probes would exercise
# the committed checker rather than the one being edited -- the opposite of what
# they are for.
probe::install_checker

CHANGES=documentation/architecture/openspec/changes

probe() {  # probe <change-id>; delta tree already built by caller
    echo
    echo "########## $1"
    probe::audit "$1"
}

# 1. A delta whose target is not live because another change has not synced it.
#    Expect: an ordering constraint naming the producer, and exit zero.
mkdir -p "$CHANGES/zz-probe-waits/specs/delivery-quiescence"
cat > "$CHANGES/zz-probe-waits/specs/delivery-quiescence/spec.md" <<'DELTA'
## MODIFIED Requirements

### Requirement: Mailbox Payload Custody

The relay SHALL do something this probe does not care about.

#### Scenario: A probe scenario

- **WHEN** this probe runs
- **THEN** the audit reports an ordering constraint rather than an error
DELTA
probe zz-probe-waits

# 2. The same shape, naming a requirement no change anywhere supplies.
#    Expect: an error, and exit one.
mkdir -p "$CHANGES/zz-probe-orphan/specs/delivery-quiescence"
cat > "$CHANGES/zz-probe-orphan/specs/delivery-quiescence/spec.md" <<'DELTA'
## MODIFIED Requirements

### Requirement: Requirement No Change Anywhere Supplies

The relay SHALL do something this probe does not care about.

#### Scenario: A probe scenario

- **WHEN** this probe runs
- **THEN** the audit still reports an error
DELTA
probe zz-probe-orphan

# 3. A requirement moved between capabilities: intact, truncated, and
#    duplicated (added without the matching removal).
python3 - <<'PYTHON'
import pathlib, re
SPECS = pathlib.Path('documentation/architecture/openspec/specs')
CHANGES = pathlib.Path('documentation/architecture/openspec/changes')
NAME = 'Relay raww operation contract'
REQ = re.compile(r'^### Requirement:[ \t]*(.+?)[ \t]*$', re.M)
STOP = re.compile(r'^(?:### Requirement:|## )', re.M)

text = (SPECS / 'raww' / 'spec.md').read_text()
header = next(m for m in REQ.finditer(text) if m.group(1) == NAME)
stop = STOP.search(text, header.end())
intact = text[header.start():stop.start() if stop else len(text)].rstrip('\n')
truncated = '#### Scenario:'.join(intact.split('#### Scenario:')[:-1]).rstrip('\n')

def build(change_id, block, with_removal):
    dest = CHANGES / change_id / 'specs' / 'zz-probe-destination'
    dest.mkdir(parents=True)
    (dest / 'spec.md').write_text(f'## ADDED Requirements\n\n{block}\n')
    if with_removal:
        source = CHANGES / change_id / 'specs' / 'raww'
        source.mkdir(parents=True)
        (source / 'spec.md').write_text(
            f'## REMOVED Requirements\n\n### Requirement: {NAME}\n\n'
            '**Reason**: Relocated by this probe.\n')

build('zz-probe-move-intact', intact, True)
build('zz-probe-move-truncated', truncated, True)
build('zz-probe-move-duplicate', intact, False)
PYTHON
probe zz-probe-move-intact
probe zz-probe-move-truncated
probe zz-probe-move-duplicate

echo
echo "expected: waits reports an ordering constraint; orphan errors;"
echo "          move-intact is silent; move-truncated shows a diff;"
echo "          move-duplicate errors on the duplicate definition"
