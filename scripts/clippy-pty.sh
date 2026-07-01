#!/usr/bin/env bash
#
# Pre-commit hook helper: clippy the `pty` Cargo feature with graceful
# skip when Zig 0.15.x is not on PATH.
#
# Rationale: the default `cargo-clippy` hook intentionally drops
# `--all-features` so contributors without Zig and outbound network
# can lint the default workspace. The Pty transport (`src/pty/`,
# `src/bin/agentmux_pty.rs`, `Cargo.toml`) opts into Zig-vendored
# libghostty-vt; this hook exercises that path only when Pty sources
# are touched (see the pre-commit.yaml `files:` filter on the parent
# hook).
#
# Behavior:
#   * zig 0.15.x on PATH      -> `cargo clippy --all-targets --features pty -- -D warnings`
#   * zig missing OR < 0.15   -> warn to stderr, exit 0
#
# Set CLIPPY_PTY_REQUIRE=1 to fail-closed instead of skipping.

set -uo pipefail

require_mode=0
if [[ "${CLIPPY_PTY_REQUIRE:-0}" == "1" ]]; then
    require_mode=1
fi

if ! command -v zig >/dev/null 2>&1; then
    echo "clippy-pty: zig not on PATH; skipping (CLIPPY_PTY_REQUIRE=1 to require)" >&2
    if [[ "$require_mode" == "1" ]]; then
        exit 1
    fi
    exit 0
fi

zig_version="$(zig version 2>/dev/null || true)"
if [[ -z "$zig_version" ]]; then
    echo "clippy-pty: zig present but 'zig version' failed; skipping" >&2
    if [[ "$require_mode" == "1" ]]; then
        exit 1
    fi
    exit 0
fi

# Strip a leading "0.15.2-extra" suffix; we want the leading dotted triplet.
zig_major="$(printf '%s' "$zig_version" | awk -F. '{print $1}')"
zig_minor="$(printf '%s' "$zig_version" | awk -F. '{print $2}')"

if [[ -z "$zig_major" || -z "$zig_minor" || "$zig_major" -lt 1 ]]; then
    # 0.x.y is the pre-1.0 series; libghostty-vt-sys 0.2.0 requires Zig 0.15.x.
    if [[ "$zig_major" == "0" && "$zig_minor" -lt 15 ]]; then
        echo "clippy-pty: zig $zig_version is older than required 0.15.x; skipping" >&2
        if [[ "$require_mode" == "1" ]]; then
            exit 1
        fi
        exit 0
    fi
fi

cargo clippy --all-targets --features pty -- -D warnings