## Why

`users.toml` is the identity and policy file — it defines global user sessions
and their policy references. It also currently carries `default-bundle`, which
is not an identity or a policy but a UI-surface operational preference (which
bundle the operator's TUI browses by default). That category mismatch was
inherited from the earlier `tui.toml` rename; it means an operator editing a
surface preference must open the identity file, and it blocks a clean home for
future surface defaults (theme, default screen mode).

## What Changes

- Introduce a new operator configuration file `ui.toml`, a sibling of
  `users.toml` under the same configuration root, for UI-surface operational
  defaults. It houses `default-bundle` as its first key.
- Move `default-bundle` out of `users.toml`: the TUI bundle-resolution
  precedence reads the configured default from `ui.toml` instead. **BREAKING**
  for anyone who set `default-bundle` in `users.toml` (alpha software — no
  compatibility shim; the field is deleted from the `users.toml` schema and
  `deny_unknown_fields` will reject it there).
- Keep `default-session` and the `[[sessions]]` identity/policy entries in
  `users.toml`. `default-session` selects *which* configured global identity the
  surface acts as, so it stays cohesive with the identities it chooses among;
  `ui.toml` is reserved for surface preferences that are not identity/policy.
- Update the shipped example configuration: drop the commented `default-bundle`
  from `data/configuration/users.toml` and add a `data/configuration/ui.toml`
  with a commented `default-bundle` example.

## Capabilities

### New Capabilities
- `ui-surface-configuration`: the `ui.toml` operator configuration file — its
  location under the configuration root, its schema (starting with the optional
  `default-bundle` key), read-only loading with `deny_unknown_fields`, and its
  role as the home for UI-surface operational defaults distinct from identity
  and policy.

### Modified Capabilities
- `cli-surface`: the one-shot CLI bundle-resolution precedence sources the
  configured `default-bundle` from `ui.toml` rather than `users.toml`/`tui.toml`.
- `tui-surface`: the interactive TUI browsing-bundle precedence sources the
  configured `default-bundle` from `ui.toml`.
- `runtime-bootstrap`: the TUI-session resolution precedence sources the
  configured `default-bundle` from `ui.toml`; `users.toml` no longer carries it.

## Impact

- Configuration: new `UiConfiguration` type + `ui.toml` loader
  (`src/configuration/`), removal of `default_bundle` from `TuiConfiguration`.
- Runtime resolution: `src/runtime/tui_session.rs` bundle-name and
  browsing-bundle resolution read `default-bundle` from the new `ui.toml` source.
- Docs: `src/tui/README.md`, `src/runtime/README.md`, `documentation/usage/`.
- Shipped config: `data/configuration/users.toml` and new
  `data/configuration/ui.toml`.
- No relay, MCP, or transport surface is affected.
