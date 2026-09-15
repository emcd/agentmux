## Context

The 0.10.0 interpolation change (`coder-command-session-interpolation`,
archived) leaves `{{session-directory}}` under a standalone-word rule:
it must be bounded on both sides by start/end or unquoted unescaped
whitespace, enforced by the POSIX shell scanner in
`src/configuration/targets.rs` (`scan_template`,
`classify_template_placeholders`, `apply_substitutions`). The accepted
vocabulary is three double-brace names; the scanner detects both brace
shapes but accepts only doubles. `RawBundleFile` carries
`format-version`, `autostart`, `groups`, `environment`, `sessions`;
`RawSession` carries `id`, `name`, `directory`, `policy`, `coder`,
`coder-session-id`, `environment`, and the ui/pubsub markers — both
with `deny_unknown_fields`, so new keys are explicit schema additions.

## Goals / Non-Goals

**Goals:**

- `{{session-directory}}` composes inside unquoted words with literal
  affixes and adjacent variables, keeping one-word argv results.
- `{{project-name}}` resolves by the stated precedence and substitutes
  raw under a grammar that keeps it shell-safe everywhere.
- Quoted, escape-continued, and quote-adjacent directory placements
  keep failing load exactly as today.

**Non-Goals:**

- General template language, ACP command rendering, v0.11
  adapter/runtime work, the Frontend-owned CLI session-resolution
  defect.

## Decisions

**D1 — Composition with an explicit affix grammar, not open literals.**
The scanner keeps tracking quote, boundary, and escape state, but the
directory placement check drops the entry-`AtBoundary` and
right-whitespace requirements in favor of a lexical rule over the
placeholder's whole word: every literal affix character must belong to
the shell-literal-safe set (ASCII letters and digits plus
`-_.:/=,+@%`), every adjacent placeholder must be a grammar-safe
variable (`{{bundle-session-id}}`, `{{project-name}}`, or another
quoted `{{session-directory}}`) — never `{{coder-session-id}}`,
whose value is only trimmed, never charset-constrained, and could
inject whitespace, expansions, or quotes into the composed word —
and the
byte after the closing `}}` must not be a quote character or an
escape. Quoted regions, active entry escapes, glob characters
(`*?[]`), expansions (`` $ ``, backquote), shell operators
(`;|&<>(){}`), comment/tilde/history markers (`#~!`), and
caret are rejected wherever they touch the word. For every `=` in the
placeholder's word, the word text from its start through that `=`,
read with each placeholder span as one ASCII letter, must not match
`^[A-Za-z_][A-Za-z0-9_]*=` — substituted values are dynamic
(`{{project-name}}` could resolve to `ROOT`), so static literal text
alone cannot prove safety, and the check must see past placeholders
to the rendered shape. The motivating `--dir=` is safe because `-`
proves the shape impossible. This conservatively rejects
mid-command env-style `CONFIG={{session-directory}}` too (harmless
as an argument, indistinguishable statically); such call sites use
the flag or separate-word form. In command position the shell would
assign rather than pass an argument, while the Pty tokenizer would
pass it through.
Rationale: the corruption hazard was never just quotes — any shell
metacharacter in unquoted context can split, expand, or resequence
the word on tmux while tokenizing literally on Pty. An enumerated
safe set keeps the parser small (one predicate over the word span)
while generalizing the motivating `--dir=` and `--mount NAME:DIR`
forms. Alternative (enumerate accepted shapes) rejected: the affix
predicate covers the same forms plus their obvious neighbors with
no extra machinery.

**D2 — Project names substitute raw under a bundle-superset grammar.**
Admitted characters are ASCII alphanumerics plus `-`, `_`, and `.`,
excluding the exact `.` and `..` segments, with no length cap and no
first-character restriction. Rationale: the grammar is a strict
superset of the path-safe canonical bundle ids — `is_valid_bundle_name`
minus the exact `./..` segments, which that predicate leaves to path
consumers — so a rule named `bundle-name` accepts every canonical
bundle id usable as a path segment, including dotted and long ones,
by construction. Dots are shell-inert inside words;
the length cap and first-char rule from session ids are dropped
because project names are mount/path identifiers, not lookup keys,
and keeping them would make `bundle-name` reject legal bundles.
A leading `-` or `.` is preserved literally; flag-vs-value
interpretation at argv position belongs to the consuming harness,
not the renderer. Alternative (keep the session-id grammar and
document `bundle-name` as partial) rejected: a rule that rejects
the names it names is the more surprising API.

**D3 — Resolution precedence with explicit-override-wins, derived
lazily.**
Session `project-name` beats everything; else the applicable
`project-name-from` rule (session-level beats bundle-level);
else the session-directory basename. A session declaring both its
own `project-name` and `project-name-from` is ambiguous and fails
load — an explicit value plus an explicit derivation rule cannot
both govern. A session `project-name` above a bundle-only
`project-name-from` is ordinary precedence (override wins), not
ambiguity. Derived values (rule yields and the basename fallback)
are resolved and grammar-validated only when the chosen tmux/Pty
template actually uses `{{project-name}}`; explicit `project-name`
values and `project-name-from` rule names validate eagerly.
Rationale: eager basename validation would fail existing bundles
whose directories were legal before project names existed —
dotted, spaced, hidden, root, or long basenames — even when no
template uses the variable. Laziness keeps the change additive:
sessions that never mention `{{project-name}}` load exactly as
before, including coder-less sessions, which have no command
template at all. Alternative (eager global validation) rejected:
it is a breaking migration masquerading as a default.

**D4 — Basename derivation is lexical on the declared directory.**
The fallback (and `session-directory-basename`) is the final path
segment of the declared `directory` string, validated against the
project-name grammar like any other source — but only when the
chosen template uses `{{project-name}}` (see D3). A non-conforming
basename fails load rather than sanitizing silently. Rationale:
rendering must equal what the operator declared; silent rewrites
would label mounts with names matching no directory. `bundle-name`
uses the canonical bundle id, which satisfies the superset grammar
by construction.

## Risks / Trade-offs

- [Composed words widen the accepted template set] → Previously
  rejected templates (`--dir={{session-directory}}`) now resolve.
  That is the feature, not a regression: every newly accepted form
  satisfies the affix grammar plus the quote/escape/assignment
  rules, so it produces exactly one argv word on both consumers
  by construction.
- [Basename may violate the grammar] → Fails load with a named
  error, but only when a template actually uses `{{project-name}}`;
  otherwise the session loads exactly as before. The operator sets
  an explicit `project-name` to proceed. No silent fallback.
- [`project-name-from` at two levels] → Session level beats bundle
  level; documented in the precedence scenarios.

## Migration Plan

None required. Additive: all previously valid templates render
byte-identically. No state, schema-version bump (new keys are
optional), or persisted artifacts.

## Open Questions

1. D3's same-level reading of "reject both" (session value +
   session rule reject; session value over bundle rule wins) goes
   one step beyond the todo text, which does not name a session-level
   `project-name-from`. Confirm the intended level split, or
   collapse the rule to bundle-only (then "both" can only mean
   session value + bundle rule, and precedence vs rejection needs a
   ruling the other way).
