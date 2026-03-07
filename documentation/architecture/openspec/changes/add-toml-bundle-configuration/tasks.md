## 1. Implementation

- [ ] 1.1 Replace JSON bundle loader with TOML loaders for
      `coders.toml` and `bundles/<bundle-id>.toml`, and add raw
      serde-mapped structs for kebab-case keys.
- [ ] 1.2 Validate TOML configuration invariants:
      unique coder IDs, unique session IDs per bundle,
      unique session names per bundle, and valid references from session
      `coder` to `[[coders]].id` in `coders.toml`.
- [ ] 1.3 Implement startup command template resolution using coder templates:
      use `resume-command` when `coder-session-id` is set, otherwise use
      `initial-command`; fail on unresolved placeholders.
- [ ] 1.4 Update reconciliation and routing code paths to use session `name`
      as tmux routing primitive while preserving current relay behavior.
- [ ] 1.5 Preserve existing optional session metadata used by current features
      (for example `display-name`) and support coder-scoped prompt-readiness
      templates.
- [ ] 1.6 Implement default config file discovery rules:
      debug repository-local `.auxiliary/configuration/tmuxmux/`
      and release `~/.config/tmuxmux/`, with explicit overrides still
      taking precedence.
- [ ] 1.7 Remove JSON bundle file assumptions from tests and rewrite fixtures
      to TOML.
- [ ] 1.8 Update README and smoke-test docs to describe TOML schema
      (`coders.toml` + `bundles/<bundle-id>.toml`) and command-template
      behavior.

## 2. Validation

- [ ] 2.1 Run `cargo check --all-targets --all-features`.
- [ ] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] 2.3 Run `cargo test --all-features`.
- [ ] 2.4 Run `openspec validate add-toml-bundle-configuration --strict`.
