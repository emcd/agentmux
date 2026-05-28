## 1. Contract Design

- [x] 1.1 Add `hosted: bool` to `ListedBundle` payload contract.
- [x] 1.2 Lock hosting predicate: probed from agentmux-owned tmux sessions
      intersected with configured tmux members; ACP-only bundles default
      `hosted=true`.
- [x] 1.3 Lock independence from `state`/`startup_health`/`state_reason_code`.

## 2. Implementation

- [x] 2.1 Add `hosted` field to `ListedBundle` in `src/relay/contract.rs`.
- [x] 2.2 Expose `bundle_hosted` helper in `src/relay/lifecycle.rs` for
      `handle_list` to probe.
- [x] 2.3 Populate `hosted` in `handle_list` from probed tmux owned-session
      intersection with the bundle's configured tmux members.

## 3. Tests

- [x] 3.1 Update `tests/integration/cli/list.rs` fake-relay fixtures so the
      new `hosted` field is populated; assert payload `hosted` value.
- [x] 3.2 Add unit regression coverage:
      - tmux-bearing bundle without owned sessions -> `hosted=false`.
      - ACP-only bundle -> `hosted=true`.
- [x] 3.3 Add integration round-trip test
      (`tests/integration/session_relay_list.rs`):
      hosted=false -> reconcile -> hosted=true -> shutdown -> hosted=false.

## 4. Spec

- [x] 4.1 Add ADDED requirement delta to `session-relay` capability in
      `openspec/changes/add-bundle-hosted-in-list-payload/specs/session-relay/spec.md`.

## 5. Validation

- [x] 5.1 Run `openspec validate add-bundle-hosted-in-list-payload --strict`
      (passed).
- [x] 5.2 Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
      and `cargo test` (all clean; 18 lib / 199 integration / 159 unit).
