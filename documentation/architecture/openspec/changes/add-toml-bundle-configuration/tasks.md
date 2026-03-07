## 1. Implementation

- [x] 1.1 Replace JSON bundle loader with TOML loaders for
      `coders.toml` and `bundles/<bundle-id>.toml`, and add raw
      serde-mapped structs for kebab-case keys.
- [x] 1.2 Validate TOML configuration invariants:
      unique coder IDs, unique session IDs per bundle,
      unique optional session `name` values per bundle, and valid references from session
      `coder` to `[[coders]].id` in `coders.toml`.
- [x] 1.3 Implement startup command template resolution using coder templates:
      use `resume-command` when `coder-session-id` is set, otherwise use
      `initial-command`; fail on unresolved placeholders.
- [x] 1.4 Update reconciliation and routing code paths to use session `id`
      as tmux routing primitive while preserving current relay behavior.
- [x] 1.5 Preserve existing optional session metadata used by current features
      (session `name`) and support coder-scoped prompt-readiness
      templates.
- [x] 1.6 Implement default config file discovery rules:
      debug repository-local `.auxiliary/configuration/tmuxmux/`
      and XDG/home fallback (`$XDG_CONFIG_HOME/tmuxmux` or `~/.config/tmuxmux`),
      with explicit overrides still
      taking precedence.
- [x] 1.7 Remove JSON bundle file assumptions from tests and rewrite fixtures
      to TOML.
- [x] 1.8 Update README and smoke-test docs to describe TOML schema
      (`coders.toml` + `bundles/<bundle-id>.toml`) and command-template
      behavior.
- [x] 1.9 Add optional `prompt-idle-column` in coder TOML templates and carry
      it through prompt-readiness gating for delivery.

## 2. Validation

- [x] 2.1 Run `cargo check --all-targets --all-features`.
- [x] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 2.3 Run `cargo test --all-features`.
- [x] 2.4 Run `openspec validate add-toml-bundle-configuration --strict`.
- [x] 2.5 Add/adjust tests covering prompt idle-column gating behavior.
