# Tasks

## 1. Configuration types and loader

- [x] 1.1 Add `UiConfiguration { default_bundle: Option<String> }` in
      `src/configuration/` (kebab-case serde, `deny_unknown_fields`), with a raw
      layer if the module splits raw/typed like `TuiConfiguration`.
- [x] 1.2 Add `load_ui_configuration(configuration_root)` mirroring
      `load_tui_configuration` (read-only, missing file → `Ok(None)`, malformed
      → structured error). Load from `<config-root>/ui.toml` only (no override
      path this change — see design O1).
- [x] 1.3 Remove `default_bundle` from `TuiConfiguration` (delete the field
      outright; no rejection logic — `deny_unknown_fields` covers a stray key).
- [x] 1.4 Public re-exports for `UiConfiguration` / `load_ui_configuration`
      alongside the existing TUI configuration exports.

## 2. Runtime resolution

- [x] 2.1 In `src/runtime/tui_session.rs`, load `ui.toml` alongside the TUI
      configuration and thread the resolved `default-bundle` into
      `resolve_bundle_name` (strict) and `resolve_browsing_bundle` (lenient),
      replacing the `TuiConfiguration.default_bundle` reads.
- [x] 2.2 Keep `default-session` resolution sourced from `users.toml`
      (`TuiConfiguration`); confirm the debug/testing `overrides/users.toml`
      precedence is unchanged and applies only to `users.toml`.
- [x] 2.3 Update the fail-fast error text that names the source file
      (`bundle is required via --bundle or users.toml default-bundle` →
      `ui.toml default-bundle`). This text is on the strict one-shot
      (`agentmux send`) path only; interactive `agentmux tui` stays lenient.
- [x] 2.4 Wire `load_ui_configuration` into `agentmux check configuration`
      (`src/commands/check.rs`) at the config-root level, alongside the existing
      `relay.toml` validation, so a malformed `ui.toml` is reported pre-flight
      with its path and field-level detail (read-only; no scaffolding).

## 3. Shipped configuration

- [x] 3.1 Remove the commented `default-bundle` from
      `data/configuration/users.toml`.
- [x] 3.2 Add `data/configuration/ui.toml` with a commented `default-bundle`
      example and a short header comment. Also wired it into the starter
      (`src/runtime/starter.rs`) alongside `relay.toml` so a fresh install
      scaffolds a fully-commented `ui.toml` — parity with the other templated
      artifacts (starter test updated to assert it).

## 4. Documentation

- [x] 4.1 Update `src/runtime/README.md` and `src/tui/README.md` bundle-
      precedence prose to source `default-bundle` from `ui.toml`. Also updated
      the root `README.md` (Association / Configuration / Starter files
      sections) per AuxFE review — it still named the removed `tui.toml`.
- [x] 4.2 Update `documentation/usage/` where `default-bundle`/`users.toml`
      bundle defaults are described. (No-op: `documentation/usage/` describes
      picker interaction and routing, not `default-bundle` config sourcing —
      nothing to change there.)

## 5. Tests

- [x] 5.1 Unit: `ui.toml` load — valid `default-bundle`, absent file → no
      default, malformed → structured error.
- [x] 5.2 Behavioral: TUI/CLI bundle resolution sources `default-bundle` from
      `ui.toml` (present → resolved; absent → strict `send` error / lenient
      `tui` fallback), exercised through the public resolution entrypoints.
- [x] 5.3 Behavioral: `agentmux check configuration` exits non-zero and names
      the file when `ui.toml` is malformed; exits zero when it is valid or
      absent.
- [x] 5.4 Behavioral: the debug `overrides/users.toml` override affects identity
      resolution only and does NOT supply a `ui.toml` `default-bundle` (O1
      root-only decision).

## 6. Specs

- [x] 6.1 On archive, add the `ui-surface-configuration` spec and apply the
      `cli-surface`, `tui-surface`, and `runtime-bootstrap` deltas to `specs/`.
