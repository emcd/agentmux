## 1. Configuration schema (raw + resolved)

- [x] 1.1 Add `environment: Vec<NameValueEntry>` (`#[serde(default)]`) to
  `RawCoder` in `configuration/raw.rs`.
- [x] 1.2 Add `environment: Vec<NameValueEntry>` to `RawBundleFile` and
  `RawSession` in `configuration/raw.rs`.
- [x] 1.3 **BREAKING**: remove `environment` from `RawAcpTarget` (raw) and
  `AcpTarget` (validated) in `configuration/raw.rs`, and from
  `AcpTargetConfiguration` in `configuration/types.rs`. Keep `headers`.
  (Also lifted the validated coder-level env onto the internal `Coder` struct
  so the loader merge can read it.)
- [x] 1.4 Add `environment: Vec<NameValueEntry>` to the resolved `BundleMember`
  in `configuration/types.rs` (`skip_serializing_if = "Vec::is_empty"`),
  carrying the merged result.

## 2. Merge + validation

- [x] 2.1 Implement a merge helper (session > bundle > coder, per-variable
  most-specific-wins, union of distinct names) and populate
  `BundleMember.environment` during resolution. The merge (`merge_environment`)
  lands in `configuration/loaders.rs` `validate_loaded_configuration`, where the
  coder (via the `coders` map), bundle, and session layers are all in scope;
  `targets.rs::build_session_target` is left unchanged (it never sees bundle
  env). Declaration order is deterministic (first-appearance position kept).
- [x] 2.2 Validate each new environment location (coder in `validate_coders`,
  bundle + session in `validate_loaded_configuration`) at load. Added an
  env-specific `validate_environment_entries` (`context` phrase: `coder '<id>'`
  / `bundle '<name>'` / `session '<id>'`): a `name` must be a usable OS env key
  — non-empty and free of `=` and NUL (an invalid name would otherwise panic
  `Command::env` / malform tmux `-e KEY=VALUE` at spawn); a `value` may be empty
  but not contain NUL. ACP `headers` keep the looser `validate_name_value_entries`
  (both fields non-empty). (Env-name validation + empty-value allowance per RG
  full-slice review of 0e75ca4.)

## 3. Transport application

- [x] 3.1 ACP: in `acp/persistent_runtime.rs`
  `initialize_persistent_acp_worker_runtime`, source the merged
  `target_member.environment` instead of the removed `&acp.environment` when
  calling `AcpStdioClient::spawn`.
- [x] 3.2 Pty: apply `self.target_member.environment` via `CommandBuilder::env`
  at spawn in `pty/transport.rs`, after the `TERM`/`COLORTERM` sets so an
  explicit operator override wins. **[Pty Specialist review]**
- [x] 3.3 Tmux: apply `member.environment` via `new-session -e KEY=VALUE` at the
  coder session-creation call site in `tmux/lifecycle.rs` `create_member_once`
  (the actual `new-session` site; there is no `tmux/pane.rs` new-session path).

## 4. Tests

- [x] 4.1 Config load/validation tests (`tests/unit/config/{coder,environment}.rs`):
  environment accepted at coder, bundle, and session levels; empty name rejected
  (bundle), a `=`-bearing name rejected (coder), and an empty `value` accepted
  (session) — matching the env-specific validator.
- [x] 4.2 Merge precedence tests (`tests/unit/config/environment.rs`): session
  overrides bundle overrides coder for a colliding name; distinct names union
  across the three levels; deterministic order asserted.
- [x] 4.3 Per-transport application:
  - ACP command-spawn: covered end-to-end by the existing ACP integration suite
    (`tests/integration/acp/*`), which drives its stub entirely through the
    coder-level `environment` (now `[[coders.environment]]`) — every ACP test
    is a live proof the merged env reaches the spawned child. Inert-on-`http`
    holds structurally (the `http` channel returns not-implemented before spawn).
  - Tmux: `relay_creates_tmux_session_with_environment_flags` in
    `tests/integration/relay_delivery_runtime.rs` boots an autostart bundle with
    bundle-level env and asserts the fake tmux's recorded `new-session` argv
    carries `-e KEY=VALUE`.
  - Pty: `pty_transport_runtime_child_env_propagates_operator_overrides`
    (`tests/unit/pty_transport.rs`, `#[cfg(feature = "pty")]` `#[ignore]`-gated,
    commit `70b269f` -> master `6e9f336`) spawns a real child and asserts
    `BundleMember.environment` reaches it (`FOR_TEST_OPERATOR_KEY`), plus
    operator TERM/COLORTERM overrides winning over the transport defaults.
    RG signed off twice (initial pass + a medium-finding amend requiring the
    startup-failure path to hard-fail via `panic!` rather than silently
    succeed, so the test can't pass without exercising the real contract).
    The ghostty pin (`fdbf9ff3`) turned out to be transient, not dead —
    confirmed via three independent clean builds — so this landed without
    further blockers.
- [x] 4.4 Rewrote the ACP-environment fixtures to the new coder-level key:
  `tests/unit/config/coder.rs` (`loads_coder_environment_onto_member`, asserting
  `member.environment`) and the `tests/integration/acp/helpers.rs` fixture
  builder (`[[coders.acp.environment]]` → `[[coders.environment]]`).

## 5. Docs + cross-reference

- [x] 5.1 `src/configuration/README.md` is a modules/invariants overview and
  does not enumerate the coder/bundle/session schema surface — no update needed.
- [x] 5.2 Commit message flags the **BREAKING** ACP-environment lift and cites
  `agents-common:todos/template/9`. (Applied at commit time; confirmed in
  `f66bf53`'s message, merged to master as `2a44f9e`.)

## 6. Validation

- [x] 6.1 `openspec validate add-configurable-environment-variables --strict` —
  valid.
- [x] 6.2 `cargo nextest run --locked` green (671 passed); clippy + fmt clean.
