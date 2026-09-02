#!/usr/bin/env bash
#
# Lints the generated default-binding section of the TUI usage guide against
# the binding table it is generated from.
#
# Rationale: `src/tui/actions/bindings.rs` is the single declaration of what a
# chord does, and the help overlay and the pane hint strips read it directly.
# Operator documentation cannot read it at render time, so it carries a copy --
# and a copy is exactly what drifts. Regenerating that copy and comparing is
# what makes the guide a projection of the table rather than a transcription of
# it, so a binding row changed without regeneration fails here rather than
# reaching an operator as a wrong instruction.
#
# This is a lint rather than a test because what it asserts is a property of a
# committed artifact, not behavior of the crate. As a pre-commit hook it
# inherits pre-commit's stashing of unstaged changes, so it judges what is being
# committed; run directly, it judges the working tree.
#
# The markers are not written down here. They are the first and last lines of
# what the generator emits, so the delimiter text has one definition -- in the
# generator -- and this script cannot disagree with it about where the block
# starts.
#
# What this deliberately does not check: whether binding prose was authored
# elsewhere in the guide. Every precise form of that check is a pattern over
# prose, and the guide legitimately names chords outside the block when
# discussing terminal capability. Reviewers hold that rule; this holds the
# mechanical half.
#
# `--fix` rewrites the block instead of judging it. It lives here rather than in
# the generator so that finding the block has one implementation: a splice that
# located the markers its own way could disagree with the check about which
# lines it is allowed to replace, and would then write a file that fails here.

set -uo pipefail

GUIDE='documentation/usage/tui.md'
GENERATOR='cargo run --quiet --example tui-binding-reference'

fix=0
case "${1-}" in
    '')      ;;
    '--fix') fix=1 ;;
    *)
        echo "lint-tui-binding-documentation: unrecognized argument '$1'; accepts --fix or nothing" >&2
        exit 1
        ;;
esac

if [[ ! -f "$GUIDE" ]]; then
    echo "lint-tui-binding-documentation: $GUIDE is missing; the generated binding section has nowhere to live" >&2
    exit 1
fi

if ! generated="$($GENERATOR)"; then
    echo "lint-tui-binding-documentation: the generator failed; cannot judge the committed section" >&2
    exit 1
fi

begin="$(head -n 1 <<< "$generated")"
end="$(tail -n 1 <<< "$generated")"

# Without this the comparison below passes vacuously the moment the generator
# stops emitting bindings: an empty block matching an empty block reports
# agreement about nothing. The guide's whole purpose is to list chords, so
# producing none is a failure however well it matches.
if [[ "$begin" == "$end" ]] || ! grep -q '^- `' <<< "$generated"; then
    echo "lint-tui-binding-documentation: the generator emitted no bindings, so this check would compare nothing against nothing" >&2
    exit 1
fi

begin_count="$(grep -c -x -F -- "$begin" "$GUIDE")"
end_count="$(grep -c -x -F -- "$end" "$GUIDE")"

if [[ "$begin_count" -ne 1 || "$end_count" -ne 1 ]]; then
    echo "lint-tui-binding-documentation: $GUIDE must carry exactly one generated block; found $begin_count opening and $end_count closing markers" >&2
    echo "lint-tui-binding-documentation: the markers are" >&2
    echo "    $begin" >&2
    echo "    $end" >&2
    exit 1
fi

committed="$(awk -v begin="$begin" -v end="$end" '
    $0 == begin { inside = 1 }
    inside      { print }
    $0 == end   { inside = 0 }
' "$GUIDE")"

# The marker counts above establish that both markers are present, not that the
# block between them is well formed. A closing marker standing ahead of the
# opening one still counts once each, and the extraction would then run from the
# opening marker to the end of the file.
if [[ "$(head -n 1 <<< "$committed")" != "$begin" || "$(tail -n 1 <<< "$committed")" != "$end" ]]; then
    echo "lint-tui-binding-documentation: $GUIDE closes the generated block before it opens it" >&2
    exit 1
fi

if [[ "$fix" -eq 1 ]]; then
    if ! awk -v begin="$begin" -v end="$end" -v replacement="$generated" '
        $0 == begin { print replacement; inside = 1; next }
        $0 == end   { inside = 0; next }
        !inside     { print }
    ' "$GUIDE" > "$GUIDE.regenerated"; then
        rm -f -- "$GUIDE.regenerated"
        echo "lint-tui-binding-documentation: could not rewrite $GUIDE" >&2
        exit 1
    fi
    mv -- "$GUIDE.regenerated" "$GUIDE"
    echo "lint-tui-binding-documentation: regenerated the binding block in $GUIDE"
    exit 0
fi

if [[ "$generated" == "$committed" ]]; then
    exit 0
fi

echo "lint-tui-binding-documentation: the generated block in $GUIDE does not match the binding table" >&2
echo "lint-tui-binding-documentation: regenerate it with" >&2
echo "    scripts/lint-tui-binding-documentation.sh --fix" >&2
echo "lint-tui-binding-documentation: expected (<) versus committed (>)" >&2
diff <(printf '%s\n' "$generated") <(printf '%s\n' "$committed") >&2
exit 1
