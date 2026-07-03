## 1. Configuration schema (raw + resolved)

- [ ] 1.1 Add `environment: Vec<NameValueEntry>` (`#[serde(default)]`) to
  `RawCoder` in `configuration/raw.rs`.
- [ ] 1.2 Add `environment: Vec<NameValueEntry>` to `RawBundleFile` and
  `RawSession` in `configuration/raw.rs`.
- [ ] 1.3 **BREAKING**: remove `environment` from `RawAcpTarget` (raw) and
  `AcpTarget` (validated) in `configuration/raw.rs`, and from
  `AcpTargetConfiguration` in `configuration/types.rs`. Keep `headers`.
- [ ] 1.4 Add `environment: Vec<NameValueEntry>` to the resolved `BundleMember`
  in `configuration/types.rs` (`skip_serializing_if = "Vec::is_empty"`),
  carrying the merged result.

## 2. Merge + validation

- [ ] 2.1 Implement a merge helper (session > bundle > coder, per-variable
  most-specific-wins, union of distinct names) and populate
  `BundleMember.environment` during resolution in `configuration/targets.rs` /
  `configuration/loaders.rs`.
- [ ] 2.2 Run `validate_name_value_entries` for each new environment location
  (coder, bundle, session) at load, mirroring the existing ACP `environment` /
  `headers` validation in `configuration/targets.rs`.

## 3. Transport application

- [ ] 3.1 ACP: in `acp/persistent_runtime.rs`
  `initialize_persistent_acp_worker_runtime`, source the merged
  `target_member.environment` instead of the removed `&acp.environment` when
  calling `AcpStdioClient::spawn`. **Compile-critical**: task 1.3's field
  removal breaks this call site.
- [ ] 3.2 Pty: apply `member.environment` via `Command::env` at spawn in
  `pty/transport.rs` (alongside the existing `TERM`/`COLORTERM` sets).
  **[Pty Specialist review]**
- [ ] 3.3 Tmux: apply `member.environment` via `new-session -e KEY=VALUE` at
  the coder session-creation call site in `tmux/pane.rs`.

## 4. Tests

- [ ] 4.1 Config load/validation tests (tests/unit): environment accepted at
  coder, bundle, and session levels; invalid `NameValueEntry` rejected at each.
- [ ] 4.2 Merge precedence tests: session overrides bundle overrides coder for
  a colliding name; distinct names union across the three levels.
- [ ] 4.3 Per-transport application tests: merged environment reaches the
  spawned child on Tmux, Pty, and ACP command-spawn; inert on ACP `http`.
- [ ] 4.4 Rewrite the existing ACP-environment fixtures to the new key:
  `tests/unit/config/coder.rs` (`loads_acp_coder_with_environment` — switch
  `[[coders.acp.environment]]` to `[coders.<id>].environment`, assert on
  `member.environment`) and the `tests/integration/acp/helpers.rs` fixture
  builder (line ~300).

## 5. Docs + cross-reference

- [ ] 5.1 Update `src/configuration/README.md` if it enumerates the coder/
  bundle/session schema surface.
- [ ] 5.2 Commit message flags the **BREAKING** ACP-environment lift and cites
  `agents-common:todos/template/9`.

## 6. Validation

- [ ] 6.1 `openspec validate add-configurable-environment-variables --strict`.
- [ ] 6.2 `cargo nextest run --locked` green; clippy + fmt clean.
