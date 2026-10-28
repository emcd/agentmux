## Why

Agentmux R&D and QA seats hardcode per-scope Cistella `[[mounts]]` tables for configuration (`~/.config/agentmux-*`) and state (`~/.local/state/agentmux-*`) directories in `agentmux.toml`. Every new relay scope means hand-maintained profile edits on every seat. Evaluation (`ideas/cistella/2`, feasible, no wire changes) showed a first third-party Cistella extension can discover these scopes dynamically at prepare time instead.

## What Changes

- Add a `cistella-extension-agentmux` workspace binary: a Cistella registered-guest executable speaking the versioned stdio wire (`hello` role advertisement, one `prepare` transaction), contributing modeless mount placements plus `(target, rights)` permissions for discovered Agentmux config/state scopes.
- Scope enumeration by host-side glob of `~/.config/agentmux*` / `~/.local/state/agentmux*` (sorted for determinism) with a closed mode rule: exact-`agentmux` config mounts `Read`, all other scopes `Full`; state always `Full`. Lexical `--deny-scope` match skips first (even for symlinks, without refusal); every remaining symlink candidate refuses prepare — no canonicalization, since canonical equality cannot prove containment (an alias to a subdirectory of protected config passes identity tests while exposing it writable). Non-directory matches are skipped. `Full` rights are deliberate same-user authority (state holds credentials and sockets by design), not a privilege boundary — only the exact-`agentmux` config `Read` is least-privilege.
- Supported host-root inputs: the guest process's own seat ambient (`HOME`, plus `XDG_CONFIG_HOME`/`XDG_STATE_HOME` when set) — host-level discovery inputs, distinct from the forbidden per-session profile/container-home inference. Deployments on explicit `AGENTMUX_*_DIRECTORY` roots are out of scope at incubation and stay on hardcoded mounts pending a documented migration eligibility check.
- Static `--container-home` argv for single-home seats at incubation; additive `container_home` prepare request field via a Cistella-side extension-protocol spec delta for multi-home seats (per Cistella Owner guidance; additive fields do not bump the epoch).
- Pinned guest wire safety (not inherited unchecked from the reference guest): input frame lengths bounded before allocation, prepare epoch/op/correlation/session validated before any filesystem contact, responses kept within the negotiated `max_frame`, stdout reserved for complete protocol frames with diagnostics on stderr.
- Track Cistella master during incubation; pin hello-layer refusal behavior in both directions against a fixed peer instead of pinning the epoch (`PROTOCOL_MAJOR = 2`).
- Reuse Agentmux path grammar from `src/runtime/paths.rs` (same domain as the `approots` crate, `todos/general/58`); extract a shared crate only if linking the full library proves too heavy for the spawned guest.
- Read-only discovery lifecycle: `cleanup=[]` for all pre-existing roots; no mkdir/chmod/hydration/deletion during prepare or teardown (the path grammar is reused through a read-only facade that never calls side-effecting helpers — linking the library itself executes nothing); a missing config/state parent is empty discovery for that side while other IO errors refuse distinctly; independently present config/state paths are contributed without inventing missing pairs.
- Migration is per-seat and atomic: hardcoded Agentmux mount sections come off the profile in the same edit that declares `[[extensions]] name = 'agentmux'`, else the N-extension merge refuses (fail-closed).

## Capabilities

### New Capabilities

- `cistella-extension`: the registered-guest binary — hello/prepare wire behavior, scope discovery, mode policy, argv contract, refusal semantics, and migration rule.

### Modified Capabilities

- None. Agentmux runtime behavior is unchanged; the extension only replaces profile-declared mounts with equivalent contributed placements.

## Impact

- New `src/bin/cistella-extension-agentmux.rs` (plus `[[bin]]` entry) and its unit/integration tests; no changes to relay, CLI, MCP, or TUI surfaces.
- Seat-local `registry.toml` `[[extensions]]` entry per seat (name `agentmux`, digest-pinned at promotion); `agentmux.toml` profile loses its hardcoded Agentmux mount sections.
- Cistella-side spec delta for the additive `container_home` prepare field, reviewed through the normal Tier-1/Tier-2 path; no epoch bump.
- Reviews: Reviewer General (tier-1), then Advisor (tier-2), then operator. Parallel track — not blocking the 0.11 main theme.
