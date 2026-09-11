## 1. Rendering

- [x] 1.1 Extend `render_command_template` (`src/configuration/targets.rs`)
  with session-id and directory substitution for
  `{{bundle-session-id}}` and `{{session-directory}}`, keeping `{coder-session-id}`
  behavior unchanged. Detect unknown placeholders on the template before
  substitution; never rescan substituted value bytes.
- [x] 1.2 Extend unknown-placeholder detection to the double-brace
  vocabulary (`{{name}}`) and the underscore name class, failing
  validation on any remainder.
- [x] 1.3 Render `{{session-directory}}` as a single shell-quoted word
  (POSIX single-quote escaping); substitute `{{bundle-session-id}}` raw
  (session-id charset precludes shell metacharacters).
- [x] 1.4 Reject non-Unicode session directory when the chosen template
  uses `{{session-directory}}`.
- [x] 1.5 Thread the session directory into the tmux/pty render call sites
  in `build_session_target`; ACP `command` passthrough unchanged.

## 2. Tests

- [x] 2.1 Public configuration controls for the full matrix: tmux
  initial, tmux resume, pty initial, pty resume — each substituting both
  variables; plus unknown `{{name}}` rejected; single-brace underscore
  literal rejected; non-Unicode directory with `{{session-directory}}`
  rejected; `{{session-directory}}`-free templates unaffected by
  non-Unicode directories; brace-shaped directory (`/work/{name}`)
  resolves; directory with spaces/quotes/backslashes arrives as one
  quoted word; ACP stdio `command` containing brace text passes through
  verbatim and unrendered.
- [x] 2.2 Existing suite green: full nextest run plus fmt/clippy per lane
  workflow.

## 3. Docs and specs

- [x] 3.1 Document `{{bundle-session-id}}`/`{{session-directory}}` in the maintainer
  configuration guide `coders.toml` section.
- [x] 3.2 Validate `openspec validate --all --strict` and confirm the
  `transport-contracts` delta archives cleanly.
