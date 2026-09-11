## 1. Rendering

- [x] 1.1 Extract a POSIX shell scanner (`scan_template`) recording
  per-occurrence quote state, word-boundary state, and right-boundary
  lookahead; handle backslash escapes outside and inside double quotes
  (single quotes treat backslashes literally).
- [x] 1.2 Classify occurrences from the scan: reject unknown names;
  enforce standalone-word placement for `{{session-directory}}` only
  (id tokens exempt); coordinate substitution through a small
  `render_command_template`.
- [x] 1.3 Substitute raw ids and the shell-quoted directory; never
  rescan value bytes; remove the dead non-Unicode directory branch.
- [x] 1.4 Thread the session directory into the tmux/pty render call sites
  in `build_session_target`; ACP `command` passthrough unchanged.

## 2. Tests

- [x] 2.1 Public configuration controls for the full matrix: tmux
  initial, tmux resume, pty initial, pty resume — each substituting both
  variables; plus unknown `{{name}}` rejected; single-brace underscore
  literal rejected; brace-shaped directory (`/work/{name}`) resolves;
  unknown placeholder rejected alongside a brace-shaped directory;
  placement controls for `{{session-directory}}` (standalone accept,
  single-quote reject, double-quote reject, adjacent-prefix reject,
  adjacent-suffix reject, after-escape reject, escape-then-whitespace
  accept, escaped-unknown reject, escaped-directory reject, escaped-id
  raw substitution); backslash-bearing directory value resolves; Pty argv equality
  asserted through the real `shell_words` tokenizer; tmux argv equality
  asserted through a shell seam; ACP stdio `command` containing brace
  text passes through verbatim and unrendered.
- [x] 2.2 Existing suite green: full nextest run plus fmt/clippy per lane
  workflow.

## 3. Docs and specs

- [x] 3.1 Document `{{bundle-session-id}}`/`{{session-directory}}` in the maintainer
  configuration guide `coders.toml` section, including the standalone-word
  placement rule and the tmux/pty scope note.
- [x] 3.2 Validate `openspec validate --all --strict` and confirm the
  `transport-contracts` delta archives cleanly.
