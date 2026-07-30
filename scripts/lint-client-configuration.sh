#!/usr/bin/env bash
#
# Lints the committed coder client configuration that carries an
# `agentmux host mcp` command line.
#
# Rationale: a coder template emits that command line into a committed file, so
# a `--state-directory` placed there is committed content wearing CLI intent. It
# would outrank the `AGENTMUX_STATE_DIRECTORY` the relay injects at spawn and
# silently put a child on a different relay than the one that spawned it -- the
# rendezvous failure the injection exists to prevent, reintroduced by a file
# nobody edits by hand. These artifacts are generated from a Copier template
# upstream, so this cannot stop the constraint being violated at the source; this
# repository is where it can be checked mechanically, and a regeneration that
# reintroduces the flag fails here.
#
# This is a lint rather than a test because what it asserts is a property of the
# repository, not behavior of the crate. As a pre-commit hook it inherits
# pre-commit's stashing of unstaged changes, so it judges what is being
# committed; run directly, it judges the working tree.

set -uo pipefail

# Every in-repo artifact carrying an `agentmux host mcp` command line.
#
# `coders/claude/settings.json` is deliberately absent: it carries environment,
# tool permissions, and sandbox settings, and names agentmux only as tool
# identifiers. It emits no command line, so it has nothing to guard.
ARTIFACTS=(
    '.auxiliary/configuration/coders/opencode/settings.jsonc'
    '.auxiliary/configuration/coders/codex/config.toml'
    '.auxiliary/configuration/mcp-servers.json'
)

fail=0

for relative in "${ARTIFACTS[@]}"; do
    if [[ ! -f "$relative" ]]; then
        echo "lint-client-configuration: $relative is missing; the --state-directory guard covers nothing" >&2
        fail=1
        continue
    fi

    if grep -q -- '--state-directory' "$relative"; then
        echo "lint-client-configuration: $relative must not emit --state-directory: a committed flag outranks the AGENTMUX_STATE_DIRECTORY the relay injects at spawn, sending the child to a relay that did not spawn it" >&2
        fail=1
    fi

    # Without this the check above passes vacuously once a template stops
    # emitting the command line, or renames the file out from under the list.
    # Quoting, commas and whitespace vary by artifact format, so they are
    # stripped before matching rather than matched around.
    if ! tr -d '",[:space:]' < "$relative" | grep -q 'hostmcp'; then
        echo "lint-client-configuration: $relative no longer carries an agentmux host mcp command line, so the --state-directory guard covers nothing" >&2
        fail=1
    fi
done

exit "$fail"
