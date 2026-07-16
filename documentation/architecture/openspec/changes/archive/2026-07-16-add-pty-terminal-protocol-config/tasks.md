# Tasks: Per-coder Pty terminal protocol configuration

## 1. Configuration schema

- [x] 1.1 Add `TermProtocol` enum in
      `src/configuration/types.rs`. Variants (each carrying an
      explicit serde rename to the literal TERM string it emits):
      - `Xterm256Color` -> `"xterm-256color"`
      - `XtermKitty` -> `"xterm-kitty"`
      - `Alacritty` -> `"alacritty"`
      - `Foot` -> `"foot"`
      - `WezTerm` -> `"wezterm"`
      - `Screen256Color` -> `"screen-256color"`
      Implement `TermProtocol::as_env_var(self) -> &'static str`
      returning the literal; implement `Default for TermProtocol`
      returning `Self::Xterm256Color`.
- [x] 1.2 Add `term_protocol: Option<TermProtocol>` to
      `RawPtyTarget` in `src/configuration/raw.rs` (kebab-case
      TOML key `term-protocol`, `#[serde(default)]`).
- [x] 1.3 Add `term_protocol: Option<TermProtocol>` to validated
      `PtyTarget` in `src/configuration/raw.rs`.
- [x] 1.4 Add `term_protocol: TermProtocol` to
      `config::types::PtyTargetConfiguration` in
      `src/configuration/types.rs`, with `#[serde(default =
      "term_protocol_default")]` defaulting to
      `TermProtocol::Xterm256Color`.

## 2. Validation

- [x] 2.1 No new validator code is required. Unknown
      `term-protocol` values (e.g., `"xterm-kittie"`) are
      rejected by serde's enum-variant deserializer with a
      structured "unknown variant" error. The existing
      `deny_unknown_fields` attribute on `RawPtyTarget` is
      unchanged and continues to reject typos in field names
      (e.g., `term-protocole` instead of `term-protocol`).
- [x] 2.2 The existing per-coder mutual-exclusion validator
      between `[coders.<id>.tmux]` / `[coders.<id>.acp]` /
      `[coders.<id>.pty]` is unchanged.

## 3. Transport-side wiring

- [x] 3.1 Add `term_protocol: TermProtocol` (clone of validated
      type) to `pty::transport::PtyTargetConfiguration` in
      `src/pty/transport.rs`.
- [x] 3.2 Update `src/relay/delivery/dispatch/worker.rs` to
      propagate `term_protocol` when constructing
      `pty::PtyTargetConfiguration` from
      `config::types::PtyTargetConfiguration` in both the
      `TargetConfiguration::Pty` arm and the fallback
      placeholder-default arm.
- [x] 3.3 In `PtyTransport::startup`
      (`src/pty/transport.rs:298`), replace the hardcoded
      `cmd.env("TERM", "xterm-256color")` with
      `cmd.env("TERM", self.term_protocol.as_env_var())`. Leave
      the `COLORTERM=truecolor` line unchanged.

## 4. Unit tests

- [x] 4.1 Add a `term_protocol_round_trips_to_env_var` test
      inside `tests/unit/pty_transport.rs` (or a sibling unit
      module) that:
      - constructs a `RawPtyTarget` with each supported
        `term_protocol` value and asserts the validated
        `TermProtocol` enum carries the right variant.
      - constructs a `RawPtyTarget` with an unknown
        `term_protocol` value and asserts deserialization
        fails (snaps the failure mode via
        `assert!(result.is_err())`).
      - asserts `TermProtocol::default().as_env_var() ==
        "xterm-256color"`.
- [x] 4.2 Add a `term_protocol_propagates_to_child_command` test
      that, when the Pty feature is enabled and `Zig 0.15.x`
      is on `PATH`, spawns a child command that prints its own
      TERM env var through the PTY — e.g.
      `sh -c 'printf "TERM=%s\n" "$TERM"'` — under
      `portable-pty`, reads the PTY output until the
      `TERM=<value>` line appears (or a short read timeout),
      and asserts the value matches the configured
      `term_protocol`. Reading `/proc/self/environ` from the
      test process is incorrect (it reports the test's env,
      not the spawned PTY child's); the child must print its
      own env through the PTY for the assertion to be
      meaningful. Marked `#[ignore]` for default-feature runs
      per the existing Pty test convention.
      (Landed with a bonus 4.3, `term_protocol_dependent_round_trip_through_snapshot`,
      covering the TERM-dependent-behavior round-trip through the
      snapshot path per Coordinator's dispatch.)

## 5. Documentation

- [x] 5.1 Update `README.md` Pty transport section with the new
      `term-protocol` field, the list of supported values, and
      a note that the project's TUIs (claude, codex, gemini,
      opencode) benefit from `xterm-kitty` for richer
      keybindings. Cite the Claude Code terminal-config docs
      and the opencode terminal-detection behavior as the
      rationale for the per-coder opt-in.
- [x] 5.2 Update `data/configuration/coders.toml` with a sample
      `[coders.<id>.pty]` block (commented as illustrative) so
      operators see the available values.

## 6. Validation

- [x] 6.1 `cargo test --lib` and `cargo test --tests` pass
      with no regressions. (Confirmed via `cargo nextest run`,
      canonical runner as of `todos/general/22`: 652/652 passed,
      0 skipped.)
- [x] 6.2 `cargo clippy --all-targets --no-deps` is silent.
      (Confirmed on both default and `--features pty`.)
- [x] 6.3 `cargo fmt --check` is silent.
- [x] 6.4 `openspec validate add-pty-terminal-protocol-config
      --strict` passes.
- [x] 6.5 `cargo run --bin agentmux-pty --features pty -- --term-protocol xterm-kitty /bin/sh -c 'echo TERM_VALUE=$TERM'`
      renders `TERM_VALUE=xterm-kitty` in the smoke binary's
      ratatui output (manual smoke test). The smoke binary
      gained a `--term-protocol <value>` CLI flag so an
      operator can smoke-test the same TERM value a coder is
      configured with via `[coders.<id>.pty].term-protocol`;
      the flag parses the same kebab-case strings the TOML
      config field uses and defaults to `xterm-256color`.
      Visual round-trip verified via the `pty-debug` MCP
      tooling (spawned the binary in a controlled PTY session,
      confirmed the rendered screen contains
      `TERM_VALUE=xterm-kitty`). Programmatic round-trip is
      already covered by `pty_transport_term_protocol_propagates_to_child_command`
      and `pty_transport_term_protocol_dependent_round_trip_through_snapshot`
      in `tests/unit/pty_transport.rs` (both `#[ignore]`'d,
      require `--features pty` + Zig 0.15.x on `PATH` at build
      time per the existing Pty test convention).
- [x] 6.6 Extracted to `todos/pty/10` (operator-scheduled
      live-bundle validation alongside `todos/pty/9`,
      `todos/relay/115`, and `todos/relay/116` in the joint
      session). The proposal does not gate archival on the
      joint session per the no-dual-maintenance convention.
