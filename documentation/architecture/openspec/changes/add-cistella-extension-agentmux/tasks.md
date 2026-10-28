## 1. Guest binary skeleton

- [ ] 1.1 Add `[[bin]] cistella-extension-agentmux` (`src/bin/cistella-extension-agentmux.rs`) with stdio frame helpers and hello epoch gate.
- [ ] 1.2 Implement argv parsing (`--role`, `--container-home`, repeatable `--deny-scope`) with fail-closed refusal on absent/relative home.
- [ ] 1.3 Implement bounded frame IO: pre-allocation length bound, prepare validation before discovery, negotiated-max output check, stdout-frames-only/stderr-diagnostics split, plus malformed/oversized/wrong-op refusal tests.
- [ ] 1.4 Wire scope grammar to `src/runtime/paths.rs` (decide link vs. shared-crate extraction with `todos/general/58`).

## 2. Prepare transaction

- [ ] 2.1 Implement host-side glob discovery of `~/.config/agentmux*` / `~/.local/state/agentmux*` honoring `--deny-scope`.
- [ ] 2.2 Reject symlink candidates at discovery (`lstat`, no canonicalization): lexical deny skips first, remaining symlinks refuse, plus protected-subdirectory and denied-subdirectory refusal cases; document the quiescent-host assumption.
- [ ] 2.3 Emit paired placements + permissions per scope under the closed mode rule (exact-`agentmux` config `Read`, rest `Full`).
- [ ] 2.4 Build discovery on a read-only facade over the path grammar (never call `ensure_directory_secure`, hydration, or root-preparation helpers); add before/after preservation + teardown test (fixture writers isolated — no mtime tolerance) and missing-parent vs IO-error tests.
- [ ] 2.5 Cover fail-closed prepare: discovery errors, gate failures, and serialization failures refuse non-zero.

## 3. Verification

- [ ] 3.1 Add hello unit tests in both directions (compatible converges, epoch mismatch refuses silently).
- [ ] 3.2 Add prepare tests: discovery set, mode policy, deny-scope, missing-home refusal.
- [ ] 3.3 Add fixture seat proving the merged plan equals the current hardcoded `agentmux.toml` table mount-for-mount.
- [ ] 3.4 Live re-proof of landlock + agentmux-mounts + ssh/tmpdir composition against the real merge pipeline.

## 4. Rollout artifacts

- [ ] 4.1 Ship the seat `registry.toml` snippet (single-home caveat documented) and the atomic profile migration edit.
- [ ] 4.2 Document the migration eligibility check (enumerate seat deployments, confirm default-layout roots and no symlinked scopes); explicit-root deployments stay hardcoded.
- [ ] 4.3 File the Cistella-side `container_home` additive-field delta as its own change (this change does not block on it).
- [ ] 4.4 Run `cargo fmt`, `clippy`, and the canonical `cargo nextest run` suite; `openspec validate --strict`.
