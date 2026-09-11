## 1. Rendering

- [ ] 1.1 Accept only the three double-brace names in classification;
  every single-brace occurrence (including `{coder-session-id}`)
  rejects as unknown. Scanner still detects both shapes.
- [ ] 1.2 Classify occurrences from the scan: reject unknown names;
  enforce standalone-word placement for `{{session-directory}}` only
  (id tokens exempt); coordinate substitution through a small
  `render_command_template`.
- [ ] 1.3 Substitute raw ids and the shell-quoted directory; never
  rescan value bytes.
- [x] 1.4 Thread the session directory into the tmux/pty render call sites
  in `build_session_target`; ACP `command` passthrough unchanged.

## 2. Tests

- [ ] 2.1 Public configuration controls for the full matrix: tmux
  initial, tmux resume, pty initial, pty resume — each substituting the
  double-brace variables; plus unknown `{{name}}` rejected; single-brace
  `{coder-session-id}` rejected as unknown (migration control);
  brace-shaped directory (`/work/{name}`) resolves;
  unknown placeholder rejected alongside a brace-shaped directory;
  placement controls for `{{session-directory}}` (standalone accept,
  single-quote reject, double-quote reject, adjacent-prefix reject,
  adjacent-suffix reject, after-escape reject, escape-then-whitespace
  accept, escaped-unknown reject, escaped-directory reject, escaped-id
  raw substitution); backslash-bearing directory value resolves; Pty argv equality
  asserted through the real `shell_words` tokenizer; tmux argv equality
  asserted through a shell seam; ACP stdio `command` containing brace
  text passes through verbatim and unrendered.
- [ ] 2.2 Migrate every repository-owned `{coder-session-id}` fixture
  to the double-brace form; full nextest run plus fmt/clippy per lane
  workflow.
- [ ] 2.3 Brace-exact proof: zero single-brace hits in positive
  surfaces (`data/`, production `src/`, `documentation/usage/`);
  repository-wide inventory classifies every remaining hit as
  migration history, normative rejection text, task wording, or
  negative test input.

## 3. Docs and specs

- [ ] 3.1 Document the three double-brace names in the maintainer
  configuration guide `coders.toml` section, including the standalone-word
  placement rule, the tmux/pty scope note, and the single-brace migration
  note.
- [x] 3.2 Validate `openspec validate --all --strict` and confirm the
  `transport-contracts` delta archives cleanly.
