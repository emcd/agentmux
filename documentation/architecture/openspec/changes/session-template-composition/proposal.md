## Why

Coder command templates cannot yet express the two shapes isolated
harnesses need: a directory composed into a larger unquoted word
(`--mount NAME:DIR`), and a stable shell-safe project identity derived
without wrapper scripts. Both extend the 0.10.0 interpolation
vocabulary without opening a general template language.

## What Changes

- Relax `{{session-directory}}` placement from standalone-word to
  unquoted-word composition: literal affixes from the
  shell-literal-safe set (ASCII letters/digits plus `-_.:/=,+@%`)
  and adjacent grammar-safe variables (`{{bundle-session-id}}`,
  `{{project-name}}`, another quoted directory — never the
  unconstrained `{{coder-session-id}}`) may share its word (e.g.
  `--dir={{session-directory}}`,
  `--mount {{project-name}}:{{session-directory}}`). Quoted,
  escape-continued, quote- or escape-adjacent, glob, expansion,
  operator placements, and words that could form a shell assignment
  (literal or placeholder-composed) still fail load.
- Add `{{project-name}}` to the accepted vocabulary, rendered raw
  under a bundle-superset path-segment grammar (ASCII
  alphanumerics plus `-_.`, excluding exact `./..`, no length cap,
  no first-character rule), so every canonical bundle id is
  admissible.
- Resolve project names in precedence order: session `project-name`
  explicit override, else the bundle `project-name-from` rule
  (`session-directory-basename` or `bundle-name`), else the
  session-directory basename. Rule yields and basenames derive and
  validate only when the chosen template uses `{{project-name}}`;
  explicit values and rule names validate eagerly. A session
  declaring both `project-name` and its own `project-name-from`
  is ambiguous and fails load; a session override above a bundle
   rule wins by precedence.
- Add `project-name` (session) and `project-name-from` (bundle and
  session) configuration keys with the bundle-superset grammar for
  values and the two rule names.
- Document the composed forms, the precedence, and the grammar in the
  maintainer guide; record the release notes.

## Capabilities

### New Capabilities

- (none — both items extend the existing template-resolution
  mechanism and bundle/session schema)

### Modified Capabilities

- `transport-contracts`: the "Coder Command Template Resolution"
  requirement gains the composition placement rule, the
  `{{project-name}}` variable, and project-name resolution precedence.
- `addressing-routing`: the "Bundle Membership Configuration"
  requirement gains the `project-name` session key and the
  `project-name-from` bundle/session key.

## Impact

- `src/configuration/targets.rs` (scanner boundary rule, classifier,
  substitution), `raw.rs`/`types.rs`/`loaders.rs` (new keys, grammar
  validation, precedence), `tests/unit/config/interpolation.rs`
  (composition, precedence, grammar controls).
- Additive: previously valid templates render byte-identically;
  previously rejected placements (`--dir={{session-directory}}`)
  now resolve. No spawn-path, schema-version, or ACP changes.
- Explicitly out of scope: general templating, ACP interpolation
  parity, v0.11 adapter/runtime work, `issues/cli/19` (Frontend-owned).
