## 1. Schema and Validation Implementation

- [ ] 1.1 Update bundle/session parser to support `format-version = 1` and
      `format-version = 2`.
- [ ] 1.2 Add `[[sessions]].[transport]` parsing with `kind = "tmux" | "acp"`.
- [ ] 1.3 For v2, default omitted `sessions.transport` to `kind = "tmux"`.
- [ ] 1.4 Add ACP descriptor validation:
      - `transport = "stdio" | "http"`
      - `session-mode = "new" | "load"`
      - `session-id` required for load mode
      - stdio requires `command`
      - http requires `url`
- [ ] 1.5 Keep v1 behavior compatible for existing tmux-only bundles.

## 2. Runtime Follow-Up

- [ ] 2.1 Introduce relay transport abstraction for tmux and ACP targets.
- [ ] 2.2 Keep current tmux behavior unchanged for tmux sessions.
- [ ] 2.3 Add ACP adapter spike for session lifecycle and prompt-turn mapping.
- [ ] 2.4 Define explicit behavior for look/quiescence parity when target
      transport is ACP.

## 3. Testing

- [ ] 3.1 Add unit coverage for v1/v2 session parsing and defaults.
- [ ] 3.2 Add validation tests for unsupported transport kinds and invalid ACP
      descriptors.
- [ ] 3.3 Add mixed-bundle tests (tmux + ACP sessions) for configuration load.

## 4. Validation

- [ ] 4.1 Run `openspec validate add-session-transport-union-schema --strict`.
- [ ] 4.2 Run targeted Rust tests for configuration parsing once
      implementation lands.
