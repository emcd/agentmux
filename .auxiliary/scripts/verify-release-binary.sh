#!/usr/bin/env bash
# Exercise the RELEASE binary for configuration layer resolution, bundle union,
# and green MCP startup. These paths vary by build profile, or must be shown not
# to, and the nextest suite compiles one profile so it cannot observe either.
#
# Originally written for redesign-configuration-resolution, which layered a
# fixed `overlay/` subdirectory beneath a single root. layer-configuration-roots
# replaced that with an operator-declared list of sibling roots, so checks A
# through C now pass the layers as repeated flags. Check B1 is new: with a list
# rather than a nested pair, the bundle-directory union is a distinct behavior
# from per-file shadowing and needs its own coverage.
#
# unify-state-root-resolution then deleted the build-profile branches from state
# and inscriptions root resolution, so check D was inverted: it asserted the two
# profiles diverge, and now asserts they agree and that isolation comes from an
# explicit --state-directory.
#
# Re-run whenever build-profile-dependent resolution changes.
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
echo "== D. Both profiles resolve one state root; isolation is explicit =="
# The inverse of what this check used to assert. Build profile no longer selects
# a state root, so the property worth proving is that it does not: the same
# invocation must name the same socket from a release build and a debug build.
# That is the assertion the deleted cfg!(debug_assertions) branches made
# impossible, and it still cannot be made by the nextest suite, which compiles
# one profile.
#
# Both run inside a THROWAWAY Agentmux checkout -- git repository plus a manifest
# declaring the package -- rather than inside this worktree. That is now a
# discriminating fixture rather than a convenience: it is exactly the shape the
# old Git provenance recognized, so a resurrected repository-local branch would
# fire here and split the two profiles apart.
CHECKOUT="$PWD/$ROOT/checkout"
mkdir -p "$CHECKOUT"
git -C "$CHECKOUT" init -q 2>/dev/null
printf '[package]\nname = "agentmux"\nversion = "0.0.0"\n' > "$CHECKOUT/Cargo.toml"
FAKEHOME="$PWD/$ROOT/home"; mkdir -p "$FAKEHOME"
# Deliberately not created: the default roots do not exist either, and creating
# only this one would put the named-root invocation on a different code path
# than the two it is being compared against.
NAMED_STATE="$PWD/$ROOT/named-state"
args=(list principals --namespace shadowed --as-session user@GLOBAL
      --configuration-directory "$PWD/$ROOT/override"
      --configuration-directory "$PWD/$ROOT/base")

ABS_RELEASE=$(cd "$(dirname "$RELEASE")" && pwd)/$(basename "$RELEASE")
ABS_DEBUG=$(cd "$(dirname "$DEBUG")" && pwd)/$(basename "$DEBUG")

# Routed through `run` like every other check, so the exit status is asserted
# rather than discarded: the expected socket path can appear in output that
# accompanies an unexpected failure, and a substring alone cannot tell the two
# apart. No relay runs in the throwaway checkout, which is the whole point --
# each profile names the socket it resolved while reporting the bundle down.
#
# Exit 0 is correct here and was not always so. This fixture lives under a
# `.auxiliary/temporary/...` path long enough to overflow `sun_path`, so before
# socket addressing stopped scaling with depth these invocations failed on the
# path length rather than on the absent relay -- the check passed, for the wrong
# reason. A `down`/`not_started` report is the honest outcome.
run_in_checkout() { # binary [extra args...]
  local binary="$1"; shift
  run env -u XDG_STATE_HOME HOME="$FAKEHOME" "$binary" "${args[@]}" "$@"
}
cd "$CHECKOUT" || exit 1
run_in_checkout "$ABS_RELEASE"; relout="$out"; relstatus=$status
run_in_checkout "$ABS_DEBUG";   dbgout="$out"; dbgstatus=$status
run_in_checkout "$ABS_DEBUG" --state-directory "$NAMED_STATE"
namedout="$out"; namedstatus=$status
cd "$OLDPWD" || exit 1

check  "release resolves the home state root"           "$FAKEHOME/.local/state/agentmux" "$relout"
check  "debug resolves the same home state root"        "$FAKEHOME/.local/state/agentmux" "$dbgout"
refute "release reaches no repository-local state root" ".auxiliary/state"                "$relout"
refute "debug reaches no repository-local state root"   ".auxiliary/state"                "$dbgout"
check  "an explicit state directory is honored"         "$NAMED_STATE/relay.sock"         "$namedout"
refute "the explicit root displaces the default"        "$FAKEHOME/.local/state/agentmux" "$namedout"
check  "release reports the bundle down"                "reason_code=not_started"         "$relout"
check  "debug reports the bundle down"                  "reason_code=not_started"         "$dbgout"
check  "the named root reports the bundle down"         "reason_code=not_started"         "$namedout"
status=$relstatus;   expect_status "release reports an absent relay without failing" 0
status=$dbgstatus;   expect_status "debug reports an absent relay without failing"   0
status=$namedstatus; expect_status "the named root reports an absent relay without failing" 0

echo
echo "== D1. Both profiles place inscriptions under the same state root =="
# Check D above compares the state root and the socket beneath it. The
# inscriptions root is selected separately and defaults to
# <state_root>/inscriptions, so "identical resolution across profiles" is only
# half-asserted without it -- and an inscriptions root that diverged by profile
# would split an operator's log history exactly the way the cutover note warns
# about, while every socket assertion above still passed.
#
# Driven through `host mcp` because that is a surface which configures a
# process inscriptions sink; a plain CLI query writes none. Each profile is
# given only a state directory, so the inscriptions path is derived rather than
# named, which is the behavior under test.
#
# The SAME state directory for both, run sequentially and emptied between runs,
# because the property is that identical arguments yield an identical
# destination. Two profile-specific roots could only ever be compared by their
# relative suffix, which agrees whenever the derivation is *shaped* alike --
# including when a resurrected branch pointed one profile at a different root
# entirely. Emptying between runs is what keeps the second observation from
# reading the first profile's file.
mcp_init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"verify","version":"0"}}}'
D1_STATE="$PWD/$ROOT/insc-shared"
# Sets `insc` (absolute path of the derived log, empty if none) and `inscstatus`.
# The two are separate variables rather than one echoed string because the
# process status has to survive: taking it from a function whose last command is
# the `find` would report on the search, not on the run.
inscriptions_under() { # binary
  local binary="$1"
  rm -rf "$D1_STATE"; mkdir -p "$D1_STATE"
  printf '%s\n' "$mcp_init" \
    | timeout 20 "$binary" host mcp "${LAYERS[@]}" \
        --state-directory "$D1_STATE" --default-bundle does-not-exist >/dev/null 2>&1
  inscstatus=$?
  insc=$(find "$D1_STATE/inscriptions" -type f -name '*.log' 2>/dev/null | sort | head -1)
}

inscriptions_under "$RELEASE"; relinsc="$insc"; relstatus=$inscstatus
inscriptions_under "$DEBUG";   dbginsc="$insc"; dbgstatus=$inscstatus

check "release derives an inscriptions path under its state root" "$D1_STATE/inscriptions/" "$relinsc"
check "debug derives an inscriptions path under its state root"   "$D1_STATE/inscriptions/" "$dbginsc"
if [ -n "$relinsc" ] && [ "$relinsc" = "$dbginsc" ]; then
  pass=$((pass+1)); echo "  PASS  both profiles derive the same inscriptions path"
else
  fail=$((fail+1)); echo "  FAIL  both profiles derive the same inscriptions path"
  echo "        release=$relinsc debug=$dbginsc"
fi
status=$relstatus; expect_status "release serves the protocol while deriving inscriptions" 0
status=$dbgstatus; expect_status "debug serves the protocol while deriving inscriptions"   0

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
