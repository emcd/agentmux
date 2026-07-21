## Context

`default-bundle` currently lives in `users.toml`, deserialized into
`TuiConfiguration` (`src/configuration/types.rs`) alongside `default-session`
and the `[[sessions]]` identity entries. TUI/CLI bundle resolution
(`src/runtime/tui_session.rs`) reads `configuration.default_bundle` in two
precedence chains:

- `resolve_bundle_name` (strict, one-shot CLI): `--bundle` → configured
  `default-bundle` → error.
- `resolve_browsing_bundle` (lenient, interactive TUI): `--bundle` → configured
  `default-bundle` → caller `fallback_bundle` → empty.

`users.toml` is otherwise the identity/policy file. Mixing a surface preference
into it is a category error and leaves no clean home for future surface defaults.

## Goals / Non-Goals

**Goals:**
- A dedicated `ui.toml` operator file for UI-surface operational defaults, read
  from the same configuration root as `users.toml`.
- `default-bundle` sourced from `ui.toml`; `users.toml` becomes pure
  identity/policy.
- Read-only, fail-fast loading consistent with the other config loaders
  (`deny_unknown_fields`, no scaffolding side effects).

**Non-Goals:**
- Moving `default-session` (identity selection stays in `users.toml`).
- Adding theme / default-screen-mode keys now — `ui.toml` is *designed* to hold
  them later, but this change ships only `default-bundle`.
- Any backwards-compatibility shim for `default-bundle` in `users.toml` (alpha:
  the field is deleted; `deny_unknown_fields` rejects it).

## Decisions

### D1: New file `ui.toml`, not a section in an existing file
Sibling of `users.toml` under the configuration root. A separate file keeps the
identity/policy file pure and gives surface preferences an obvious, greppable
home. Alternative considered: a `[ui]` table inside `relay.toml` or `users.toml`
— rejected, it re-creates the category mixing this change exists to remove.

### D2: `default-session` stays in `users.toml`
`default-session` chooses which configured global identity the surface acts as;
it only has meaning relative to the `[[sessions]]` entries in `users.toml`.
`default-bundle` names a bundle enumerated at runtime from the relay, unrelated
to the identities. So the split is: identity selection (`default-session`) with
the identities; surface preference (`default-bundle`) in `ui.toml`. Alternative:
move both — rejected, it would orphan `default-session` from its session list and
force `ui.toml` to reach into identity semantics.

### D3: New `UiConfiguration` type + dedicated loader
Add `UiConfiguration { default_bundle: Option<String> }`
(`serde(rename_all = "kebab-case", deny_unknown_fields)`) and
`load_ui_configuration(configuration_root)` mirroring
`load_tui_configuration`. Remove `default_bundle` from `TuiConfiguration`.
`resolve_tui_session_identity` / `resolve_tui_launch_identity` load `ui.toml`
alongside `users.toml` and pass the resolved default into the bundle-resolution
helpers. The local debug/testing override (`local_override_path`) applies to
`users.toml`; `ui.toml` loading follows the same override discipline if a
surface override is warranted — otherwise it loads from the configuration root
only. (Open question O1.)

### D4: Absent `ui.toml` is not an error
A missing `ui.toml` resolves to no configured `default-bundle` — identical to
today's absent/commented `default-bundle`. Strict one-shot resolution
(`agentmux send`) then still errors if no `--bundle` is supplied; the
interactive TUI still falls back to `fallback_bundle`/empty. Only a malformed
`ui.toml` fails fast.

### D5: Reconcile pre-existing tui bundle-resolution drift in the rewritten specs
The `runtime-bootstrap` and `cli-surface` requirements this change MODIFIES
carried stale text: `--session` (the surface is `--as-session`) and a fail-fast
bundle rule for interactive `agentmux tui`. The shipped code resolves the
interactive browsing bundle leniently (`resolve_browsing_bundle`: `--bundle` →
`default-bundle` → first available → empty) and only `agentmux send` is strict
(`resolve_bundle_name`). The live `tui-surface` spec already documents the
lenient tui behavior, so the other two specs contradicted both it and the code.
Because a MODIFIED delta replaces the whole requirement, this change corrects
both to `--as-session` and lenient-tui / strict-send rather than re-attesting
the drift. No behavior change — this only aligns the spec to shipped code.

## Risks / Trade-offs

- [Operators with `default-bundle` in `users.toml` break on upgrade] → alpha
  software with no compat guarantee; `deny_unknown_fields` gives an immediate,
  clear rejection naming the offending key, and the `agentmux check configuration`
  preflight command surfaces it before launch. Release notes call it out.
- [Two files to read for TUI startup instead of one] → both are small,
  root-relative, and loaded once at startup; negligible cost, and
  `agentmux check configuration` validates both.

## Migration Plan

1. Land the `ui.toml` loader + `UiConfiguration`, wire resolution, remove
   `default_bundle` from `TuiConfiguration` in one change.
2. Update shipped `data/configuration/users.toml` (drop commented key) and add
   `data/configuration/ui.toml` (commented example).
3. Document the move in release notes. No data migration tooling — operators
   relocate one key by hand.

Rollback: revert the change; `default-bundle` returns to `users.toml`.

## Resolved Questions

- **O1 (resolved: root-only)**: `ui.toml` loads from the configuration root
  only; it does NOT honor a parallel `.auxiliary/.../overrides/ui.toml`
  debug-override. The local override mechanism stays scoped to `users.toml`
  (identity), so debug/testing overrides affect identity only, never UI-surface
  defaults. A parallel `overrides/ui.toml` can be introduced later if a surface
  override is actually needed. AuxBE review concurred.
