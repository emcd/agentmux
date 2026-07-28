#!/usr/bin/env bash
# Exercise the RELEASE binary for configuration root resolution, overlay
# shadowing, and green MCP startup. These paths are gated on
# cfg!(debug_assertions) or otherwise vary by build profile, so the nextest
# suite -- which runs debug -- cannot observe them. Written for task 9.3 of
# redesign-configuration-resolution and kept because the gap it covers outlives
# that change.
#
# Re-run whenever build-profile-dependent resolution changes. In particular, the
# runtime-instance work removes the repository-local state and inscriptions
# branches, which is exactly what checks C and D assert; those two need
# rewriting rather than re-running once that lands.
#
# Requires a release build: cargo build --release --bin agentmux
# (Check C additionally requires a debug build.)
#
# Run from the repository root. Writes only under .auxiliary/temporary.
set -uo pipefail

RELEASE=./target/release/agentmux
DEBUG=./target/debug/agentmux
ROOT=.auxiliary/temporary/verify-9.3
rm -rf "$ROOT"; mkdir -p "$ROOT/base/bundles" "$ROOT/base/overlay/bundles"

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

echo
echo "== A. Overlay shadowing in a release build =="
# `check configuration` reports validity, not contents, so validity is the
# observable: make exactly one of the two copies invalid and see which verdict
# comes back. That proves which file was parsed rather than inferring it.
printf '%s' "$INVALID_BUNDLE" > "$ROOT/base/bundles/shadowed.toml"
printf '%s' "$VALID_BUNDLE"   > "$ROOT/base/overlay/bundles/shadowed.toml"
run "$RELEASE" check configuration shadowed --configuration-directory "$ROOT/base"
check         "valid overlay wins over an invalid base" "all valid" "$out"
expect_status "valid overlay wins over an invalid base" 0

printf '%s' "$VALID_BUNDLE"   > "$ROOT/base/bundles/shadowed.toml"
printf '%s' "$INVALID_BUNDLE" > "$ROOT/base/overlay/bundles/shadowed.toml"
run "$RELEASE" check configuration shadowed --configuration-directory "$ROOT/base"
check         "invalid overlay is reported as the fault"      "this-field-does-not-exist" "$out"
refute        "invalid overlay does not fall through to base" "all valid"                 "$out"
expect_status "invalid overlay fails the command"             1

echo
echo "== B. Base file reachable when the overlay lacks it =="
run "$RELEASE" check configuration baseonly --configuration-directory "$ROOT/base"
check         "base-only bundle resolves" "ok: baseonly" "$out"
expect_status "base-only bundle resolves" 0

echo
echo "== C. Debug and release resolve identically =="
# Discriminating by construction: base and overlay disagree about validity, so
# the two profiles produce identical output only if they select the same file.
# Comparing two VALID copies would pass whether or not resolution agreed.
printf '%s' "$INVALID_BUNDLE" > "$ROOT/base/bundles/shadowed.toml"
printf '%s' "$VALID_BUNDLE"   > "$ROOT/base/overlay/bundles/shadowed.toml"
run "$RELEASE" check configuration shadowed --configuration-directory "$ROOT/base"
rel="$out"; relstatus=$status
run "$DEBUG"   check configuration shadowed --configuration-directory "$ROOT/base"
dbg="$out"; dbgstatus=$status
if [ "$rel" = "$dbg" ] && [ "$relstatus" -eq "$dbgstatus" ]; then
  echo "  PASS  identical resolution and status across build profiles"; pass=$((pass+1))
else
  echo "  FAIL  build profiles diverge (exit $dbgstatus vs $relstatus)"
  diff <(echo "$dbg") <(echo "$rel"); fail=$((fail+1))
fi
check "both profiles selected the overlay copy" "all valid" "$rel"

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
CONFIG="$PWD/$ROOT/base"
args=(list principals --namespace shadowed --as-session user@GLOBAL
      --configuration-directory "$CONFIG")

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
  | timeout 20 "$RELEASE" host mcp --configuration-directory "$ROOT/base" \
      --default-bundle does-not-exist 2>/dev/null); status=$?
check         "initialize is answered despite the fault"     '"protocolVersion"' "$mcp"
check         "tool surface is advertised despite the fault" '"name":"send"'     "$mcp"
expect_status "process serves the protocol and exits cleanly" 0

echo
echo "== F. Green MCP startup on a missing configuration root (release) =="
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
