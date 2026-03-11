## 1. Schema and Validation Implementation

- [ ] 1.1 Update parser to require `format-version = 2` for this coder-target
      schema.
- [ ] 1.2 Extend `RawCoder` to include optional one-of target tables:
      - `tmux`
      - `acp`
- [ ] 1.3 Add one-of validation/imputation for coders:
      - exactly one target table required
      - produce non-optional `Target` on validated `Coder`
- [ ] 1.4 Add tmux coder descriptor parsing/validation in `[coders.tmux]`:
      - required `initial-command`
      - required `resume-command`
      - optional prompt-readiness keys
- [ ] 1.5 Add ACP coder descriptor parsing/validation in `[coders.acp]`:
      - `channel = "stdio" | "http"`
      - `session-mode = "new" | "load"` (default `new`)
      - stdio requires `command`
      - http requires `url`
- [ ] 1.6 Add session-to-coder constraint validation:
      - sessions reference existing coders
      - if coder `session-mode = "load"`, session must include
        `coder-session-id`

## 2. Runtime Follow-Up

- [ ] 2.1 Introduce coder-target abstraction (`tmux`, `acp`) in runtime paths.
- [ ] 2.2 Preserve existing tmux behavior under tmux coder target.
- [ ] 2.3 Add ACP adapter spike for lifecycle and prompt-turn mapping.

## 3. Testing

- [ ] 3.1 Add unit tests for coder target one-of validation.
- [ ] 3.2 Add tests for missing/multiple coder target tables.
- [ ] 3.3 Add tests for ACP load-mode requiring session `coder-session-id`.
- [ ] 3.4 Add mixed-coder bundle tests (tmux + acp coders across sessions).

## 4. Validation

- [ ] 4.1 Run `openspec validate add-session-transport-union-schema --strict`.
- [ ] 4.2 Run targeted Rust configuration tests after implementation.
