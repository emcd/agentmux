#!/usr/bin/env bash
# Exercise the RELEASE binary for configuration layer resolution, bundle union,
# and green MCP startup. These paths are gated on cfg!(debug_assertions) or
# otherwise vary by build profile, so the nextest suite -- which runs debug --
# cannot observe them.
#
# Originally written for redesign-configuration-resolution, which layered a
# fixed `overlay/` subdirectory beneath a single root. layer-configuration-roots
# replaced that with an operator-declared list of sibling roots, so checks A
# through C now pass the layers as repeated flags. Check B1 is new: with a list
# rather than a nested pair, the bundle-directory union is a distinct behavior
# from per-file shadowing and needs its own coverage.
#
# Re-run whenever build-profile-dependent resolution changes. In particular, the
# runtime-instance work removes the repository-local state and inscriptions
# branches, which is exactly what check D asserts; it needs rewriting rather than
# re-running once that lands.
#
# Requires a release build: cargo build --release --bin agentmux
# (Check C additionally requires a debug build.)
#
# Run from the repository root. Writes only under .auxiliary/temporary.
set -uo pipefail

RELEASE=./target/release/agentmux
DEBUG=./target/debug/agentmux
ROOT=.auxiliary/temporary/verify-release-binary
rm -rf "$ROOT"; mkdir -p "$ROOT/base/bundles" "$ROOT/override/bundles"

pass=0; fail=0
status=0   # exit status of the most recent run_* invocation
check() { # name, expected-substring, actual
  if grep -qF -- "$2" <<<"$3"; then
    echo "  PASS  $1"; pass=$((pass+1))
  else
    echo "  FAIL  $1"; echo "        expected to contain: $2"; echo "        actual: $3"; fail=$((fail+1))
  fi
}
refute() { # name, forbidden-substring, actual
  if grep -qF -- "$2" <<<"$3"; then
    echo "  FAIL  $1"; echo "        must not contain: $2"; echo "        actual: $3"; fail=$((fail+1))
  else
    echo "  PASS  $1"; pass=$((pass+1))
  fi
}
# Substring assertions alone cannot distinguish a command that succeeded from
# one that failed while happening to mention the expected text, so every run
# also asserts its exit status.
expect_status() { # name, expected-status
  if [ "$status" -eq "$2" ]; then
    echo "  PASS  $1 (exit $status)"; pass=$((pass+1))
  else
    echo "  FAIL  $1"; echo "        expected exit $2, got $status"; fail=$((fail+1))
  fi
}
run() { # captures stdout+stderr into $out and exit status into $status
  out=$("$@" 2>&1); status=$?
}

cat > "$ROOT/base/coders.toml" <<'EOF'
format-version = 1
[[coders]]
id = 'sh'
[coders.tmux]
initial-command = 'sh'
resume-command = 'sh'
EOF

cat > "$ROOT/base/policies.toml" <<'EOF'
format-version = 1
default = 'default'
[[policies]]
id = 'default'
description = 'Verification policy.'
[policies.controls]
find = 'all'
list = 'all'
look = 'all'
send = 'all'
EOF

cat > "$ROOT/base/users.toml" <<'EOF'
default-session = 'user@GLOBAL'
[[sessions]]
id = 'user@GLOBAL'
name = 'Verifier'
policy = 'default'
[sessions.ui]
EOF

VALID_BUNDLE="format-version = 1
[[sessions]]
id = 'member'
directory = '/tmp'
coder = 'sh'
"
INVALID_BUNDLE="format-version = 1
this-field-does-not-exist = true
"

printf '%s' "$VALID_BUNDLE" > "$ROOT/base/bundles/baseonly.toml"
printf '%s' "$VALID_BUNDLE" > "$ROOT/override/bundles/overrideonly.toml"

# The layer list, highest precedence first. Passed as repeated flags so the
# script exercises the repeatability the flag contract specifies rather than the
# environment form, which cannot express a path containing a colon.
LAYERS=(--configuration-directory "$ROOT/override" --configuration-directory "$ROOT/base")

echo
echo "== A. Layer shadowing in a release build =="
# `check configuration` reports validity as well as sources, so validity is the
# discriminator: make exactly one of the two copies invalid and see which verdict
# comes back. That proves which file was parsed rather than inferring it. The
# source line is asserted alongside, since it is the surface an operator reads.
printf '%s' "$INVALID_BUNDLE" > "$ROOT/base/bundles/shadowed.toml"
printf '%s' "$VALID_BUNDLE"   > "$ROOT/override/bundles/shadowed.toml"
run "$RELEASE" check configuration shadowed "${LAYERS[@]}"
check         "valid earlier layer wins over an invalid base" "all valid" "$out"
check         "the source names the supplying layer"          "override/bundles/shadowed.toml" "$out"
expect_status "valid earlier layer wins over an invalid base" 0

printf '%s' "$VALID_BUNDLE"   > "$ROOT/base/bundles/shadowed.toml"
printf '%s' "$INVALID_BUNDLE" > "$ROOT/override/bundles/shadowed.toml"
run "$RELEASE" check configuration shadowed "${LAYERS[@]}"
check         "invalid earlier layer is reported as the fault"      "this-field-does-not-exist" "$out"
refute        "invalid earlier layer does not fall through to base" "all valid"                 "$out"
expect_status "invalid earlier layer fails the command"             1

echo
echo "== B. Base file reachable when the earlier layer lacks it =="
run "$RELEASE" check configuration baseonly "${LAYERS[@]}"
check         "base-only bundle resolves" "ok: baseonly" "$out"
expect_status "base-only bundle resolves" 0

echo
echo "== B1. Bundle directories union across layers =="
# Distinct from shadowing: a bundle defined in only one layer must remain
# discoverable from the whole list, in both directions. Whole-directory
# replacement -- the plausible wrong implementation -- would drop whichever
# layer's bundles lost, so asserting both names in one run discriminates against
# it. Restore the shadowed pair to valid first so this measures union alone.
printf '%s' "$VALID_BUNDLE" > "$ROOT/base/bundles/shadowed.toml"
printf '%s' "$VALID_BUNDLE" > "$ROOT/override/bundles/shadowed.toml"
run "$RELEASE" check configuration "${LAYERS[@]}"
check         "a base-only bundle is in the effective set"     "ok: baseonly"     "$out"
check         "an override-only bundle is in the effective set" "ok: overrideonly" "$out"
check         "a bundle in both layers is validated once"      "checked 3 bundle configuration(s)" "$out"
expect_status "the union validates cleanly"                    0

echo
echo "== C. Debug and release resolve identically =="
# Discriminating by construction: the two layers disagree about validity, so the
# profiles produce identical output only if they select the same file. Comparing
# two VALID copies would pass whether or not resolution agreed. Both the
# comparison and the release verdict are asserted, so an outcome where the
# profiles agree on a wrong answer still fails.
printf '%s' "$INVALID_BUNDLE" > "$ROOT/base/bundles/shadowed.toml"
printf '%s' "$VALID_BUNDLE"   > "$ROOT/override/bundles/shadowed.toml"
run "$RELEASE" check configuration shadowed "${LAYERS[@]}"
rel="$out"; relstatus=$status
run "$DEBUG"   check configuration shadowed "${LAYERS[@]}"
dbg="$out"; dbgstatus=$status
if [ "$rel" = "$dbg" ] && [ "$relstatus" -eq "$dbgstatus" ]; then
  echo "  PASS  identical resolution and status across build profiles"; pass=$((pass+1))
else
  echo "  FAIL  build profiles diverge (exit $dbgstatus vs $relstatus)"
  diff <(echo "$dbg") <(echo "$rel"); fail=$((fail+1))
fi
check "both profiles selected the earlier layer's copy" "all valid" "$rel"
status=$relstatus; expect_status "release accepts the valid earlier layer" 0

echo
echo "== D. Release ignores repository-local state; debug does not =="
# Both profiles run inside a THROWAWAY Agentmux checkout -- git repository plus a
# manifest declaring the package -- rather than inside this worktree. Running
# here would let a debug build reach the live relay at the repository-local state
# root and report a routing error instead of naming a socket, which would force
# the debug half of this check to be a weak negative assertion. In a checkout
# with no relay running, both profiles name the socket they resolved, so both
# halves are positive and the gate is proven from each side.
CHECKOUT="$PWD/$ROOT/checkout"
mkdir -p "$CHECKOUT"
git -C "$CHECKOUT" init -q 2>/dev/null
printf '[package]\nname = "agentmux"\nversion = "0.0.0"\n' > "$CHECKOUT/Cargo.toml"
FAKEHOME="$PWD/$ROOT/home"; mkdir -p "$FAKEHOME"
args=(list principals --namespace shadowed --as-session user@GLOBAL
      --configuration-directory "$PWD/$ROOT/override"
      --configuration-directory "$PWD/$ROOT/base")

ABS_RELEASE=$(cd "$(dirname "$RELEASE")" && pwd)/$(basename "$RELEASE")
ABS_DEBUG=$(cd "$(dirname "$DEBUG")" && pwd)/$(basename "$DEBUG")

# Routed through `run` like every other check, so the exit status is asserted
# rather than discarded: the expected socket path can appear in output that
# accompanies an unexpected failure, and a substring alone cannot tell the two
# apart. Both profiles are expected to fail with the relay-unavailable status,
# since no relay is running in the throwaway checkout -- that failure is the
# whole point, because it is what makes each profile name the socket it
# resolved.
run_in_checkout() { # binary
  run env -u XDG_STATE_HOME HOME="$FAKEHOME" "$1" "${args[@]}"
}
cd "$CHECKOUT" || exit 1
run_in_checkout "$ABS_RELEASE"; relout="$out"; relstatus=$status
run_in_checkout "$ABS_DEBUG";   dbgout="$out"; dbgstatus=$status
cd "$OLDPWD" || exit 1

check  "release resolves the XDG state root"             "$FAKEHOME/.local/state/agentmux" "$relout"
refute "release reaches no repository-local state root"  ".auxiliary/state"                "$relout"
check  "debug resolves the repository-local state root"  "$CHECKOUT/.auxiliary/state/agentmux" "$dbgout"
refute "debug does not resolve the XDG state root"       "$FAKEHOME/.local/state/agentmux" "$dbgout"
status=$relstatus; expect_status "release exits non-zero on the unavailable relay" 1
status=$dbgstatus; expect_status "debug exits non-zero on the unavailable relay"   1

echo
echo "== E. Green MCP startup on an unknown bundle (release) =="
init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"verify","version":"0"}}}'
listt='{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
mcp=$(printf '%s\n%s\n' "$init" "$listt" \
  | timeout 20 "$RELEASE" host mcp "${LAYERS[@]}" \
      --default-bundle does-not-exist 2>/dev/null); status=$?
check         "initialize is answered despite the fault"     '"protocolVersion"' "$mcp"
check         "tool surface is advertised despite the fault" '"name":"send"'     "$mcp"
expect_status "process serves the protocol and exits cleanly" 0

echo
echo "== F. Green MCP startup on a missing configuration layer (release) =="
mcp=$(printf '%s\n%s\n' "$init" "$listt" \
  | timeout 20 "$RELEASE" host mcp --configuration-directory "$ROOT/does-not-exist" \
      --default-bundle whatever 2>/dev/null); status=$?
check         "initialize is answered"     '"protocolVersion"' "$mcp"
check         "tool surface is advertised" '"name":"list"'     "$mcp"
expect_status "process serves the protocol and exits cleanly" 0

echo
echo "----"
echo "pass=$pass fail=$fail"
[ "$fail" -eq 0 ]
