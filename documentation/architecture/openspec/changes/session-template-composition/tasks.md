## 1. Composition placement

- [ ] 1.1 Enforce the shell-literal-safe affix grammar over the
  placeholder's word (alphanumerics plus `-_.:/=,+@%`, adjacent
  grammar-safe variables only — never `{{coder-session-id}}`);
  reject glob, expansion, operator,
  comment/tilde/history, caret, quote, and escape adjacencies;
  for every `=` in the word, reject when the word text from word
  start through it — placeholders read as one ASCII letter —
  matches assignment shape; keep `--dir=` positive.
- [ ] 1.2 Keep quoted, escape-continued, and quote-adjacent
  rejections with distinguishing controls for each position,
  including immediate-right escape.
- [ ] 1.3 Prove composed words arrive as one argv element through
  both consumers (Pty tokenizer, shell seam), including the
  `--mount {{project-name}}:{{session-directory}}` form.

## 2. Project-name resolution

- [ ] 2.1 Add `project-name` (session) and `project-name-from`
  (bundle, session) keys with `deny_unknown_fields` coverage and
  the bundle-superset path-segment grammar validator (dots
  admitted except exact `./..`, no length cap, no first-char rule).
- [ ] 2.2 Implement precedence (override > applicable rule >
  directory basename) with same-level ambiguity rejection;
  derive and validate rule yields and basenames only when the
  chosen template uses `{{project-name}}`, keeping explicit
  values and rule names eager.
- [ ] 2.3 Substitute `{{project-name}}` raw in tmux/pty rendering;
  reject unknown `{{project-name}}`-adjacent misspellings through
  the general classifier.
- [ ] 2.4 Controls for every precedence tier, both rule values,
  ambiguity rejection, grammar rejection, non-conforming used
  basename failure, dotted/long bundle-name acceptance, the
  preservation case (nonconforming basename with no usage loads),
  and variable-composed assignment rejection (raw-variable and
  directory cases) with the `--dir=` positive control.

## 3. Docs and release

- [ ] 3.1 Document composed forms, precedence, grammar, and the
  standalone-to-composed migration in the maintainer guide
  `coders.toml` and bundle sections.
- [ ] 3.2 Record Added/Changed entries under CHANGELOG Unreleased.
- [ ] 3.3 Validate `openspec validate --all --strict`, delta audit,
  fmt/clippy/nextest per lane workflow.
