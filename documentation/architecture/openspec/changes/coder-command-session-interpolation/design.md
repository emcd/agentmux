## Context

Coder profile commands render per session in `render_command_template`
(`src/configuration/targets.rs:404`), reached for tmux/pty
`initial-command`/`resume-command` from `build_session_target`. The only
variable today is single-brace `{coder-session-id}`, substituted from the
session's `coder-session-id`; any other `{name}` fails load, and the resolved
command must be non-empty. ACP stdio `command` never renders — it passes
through verbatim. Every session declares a required non-empty `directory`
(`RawSession.directory`, `src/configuration/raw.rs:230`, enforced in
`loaders.rs:353`) stored as `BundleMember.working_directory`; the normalized
bare member id (`session_id`) is already a parameter of the render path.

Both new tokens fail load today: the inner `{session-directory}` and
`{bundle-session-id}` each match the unknown-placeholder regex
(`\{[a-z][a-z0-9-]*\}`), so neither is substituted. The silent-passthrough
class is narrower than hyphenated names: only placeholders whose names
carry characters outside the regex class — in practice, underscores, e.g.
`{{bundle_session_id}}` — pass through unrendered today. No `{{seat}}`,
`{{bundle}}`, or `{{harness}}` variable exists anywhere in this repository.

## Goals / Non-Goals

**Goals:**

- `{{bundle-session-id}}` and `{{session-directory}}` substitute in tmux/pty
  initial/resume commands with load-time validation.
- Any other `{{name}}` fails load, closing the silent-passthrough gap.
- The motivating correlation labels (`agentmux.session=`,
  `--directory`) become expressible in a coder profile.

**Non-Goals:**

- `{{seat}}`, `{{bundle}}`, `{{harness}}` — not added; still load errors.
- ACP stdio `command` rendering — passthrough preserved.
- No schema change to `[[coders]]`, no spawn-path change, no new config keys.

## Decisions

**D1 — Value sources: existing load-time facts, no new resolution.**
`{{bundle-session-id}}` renders the already-passed normalized bare member id
(the `session_id` parameter — `[[sessions]].id`, not the qualified
`id@bundle`); `{{session-directory}}` renders the session's declared
`directory` as the literal declared string with no rebasing or
canonicalization; the value is shell-quoted per D5 so the resulting argv
argument equals the declared directory on both transports, provided the
placeholder occupies a standalone word (see D5). Rationale: both are available in `build_session_target` without
new lookup or resolution. Naming: `bundle-session-id` reads as the
session id in bundle id-space, symmetric with `coder-session-id` (the
session id in coder id-space) — hyphens throughout, per the existing
convention, overriding the underscore spelling in `todos/runtime/26` per
operator direction. Alternative (qualified `session@bundle` as the value)
rejected: the qualifier is not known at this layer, and
bundle correlation belongs to the follow-on `{{bundle}}` variable. Likewise
`{{session-directory}}` names the session's declared directory exactly
(rather than the ambiguous `{{directory}}`), converging with Cistella's
`--session-directory` flag.

**D2 — Scope: tmux/pty initial/resume only; ACP `command` untouched.**
The example's bare `command` key matches no current coder field (tmux/pty
use `initial-command`; ACP uses `command` but never renders). Rendering ACP
`command` would newly reject previously-valid ACP commands containing
braces, a regression on a passthrough contract. Alternative (render all
three command fields) rejected for that blast radius; if the Cistella
driver needs ACP transport, it is a follow-on with its own regression
analysis. A tmux/pty coder entry serves the conduct wrapper today.

**D3 — Unified unknown-placeholder rule, extended name class.**
Classify every placeholder occurrence in the original template: each
`{{name}}` or `{name}` with `name = [a-z][a-z0-9_-]*` (underscore added to
the existing class) is either one of the three known variables
(`{coder-session-id}`, `{{bundle-session-id}}`, `{{session-directory}}`)
or unknown. Reject unknown occurrences, reject `{coder-session-id}`
occurring without a session value, and only then substitute the known
occurrences — never scanning the rendered value bytes. Rationale: one rule for both
brace shapes; underscore support is required to reject unknown doubles
like `{{bundle_session_id}}`
(a realistic typo of the hyphenated name). Consequence: single-brace
literals with underscores (silent today) also
fail — same narrow breaking class as the proposal notes, covered by a
scenario.

**D4 — Removed during implementation review.**
Non-UTF8 directory rejection was specified here, then dropped: the session
directory deserializes from a TOML string, which is Unicode by
construction, so no public configuration path can supply non-Unicode
bytes to the renderer. The number is retained as a gap so review
references to D5 stay stable.

**D5 — Interpolation semantics: validate the template, quote the directory, place it standalone.**
Unknown-placeholder detection inspects the template *before* substitution;
substituted value bytes are never rescanned, so a directory containing
brace-shaped text (e.g. `/work/{name}`) cannot be mistaken for a template
placeholder. `{{session-directory}}` renders as a single shell-quoted word
denoting the declared directory (POSIX single-quote escaping): tmux hands
the rendered string to `new-session` (shell interpretation,
`src/tmux/lifecycle.rs:164`) and pty tokenizes with `shell_words::split`
(`src/pty/command.rs`), and both honor single quotes — so quoting is
transparent for simple paths and correct for spaces, quotes, and
backslashes, arriving as one argument equal to the declared directory on
both transports. `{{bundle-session-id}}` and `{coder-session-id}`
substitute raw: session ids are charset-constrained at validation
(`validate_session_id`: ASCII alphanumeric plus `-`/`_`), so no
shell-significant character can reach the template. Alternative (raw
directory fragment with documented limitations) rejected: directories with
spaces are ordinary, and pushing word-splitting onto the operator trades a
quoting rule implementers own for failures operators cannot foresee.
`{{session-directory}}` SHALL occupy an entire unquoted shell word in the
original template: the scanner must be in Outside quote state with no
active escape at the opening `{{`, and the byte after the closing `}}`
must be end-of-template or an unquoted unescaped whitespace. Templates
placing it inside single or double quotes, adjacent to non-boundary
characters on either side, or after an escape sequence that continues the
current word SHALL fail configuration validation. This restriction
applies only to `{{session-directory}}`; the id tokens substitute raw
and may occur in any template context. The scanner walks the template as
a POSIX shell grammar (Outside / InsideSingleQuote / InsideDoubleQuote,
word-boundary tracking, backslash escape state) so apparent separators
are judged active or not; per-occurrence context (entry quote, entry
boundary, right-boundary lookahead) is recorded once and read by
classification, keeping the renderer itself small.

## Risks / Trade-offs

- [Silent-literal configurations now fail] → Mitigation: the only newly
  rejected templates are ones containing underscore-bearing placeholder
  literals (e.g. `{{bundle_session_id}}`), which today render into a
  command carrying the raw braces — almost certainly already broken at
  spawn. Called out in the proposal.
- [Bare vs qualified session id] → A bare id can collide across bundles in
  Cistella-side selectors. Mitigation: documented in the spec; `{{bundle}}`
  is the follow-on fix, and the Cistella side was always going to need
  `agentmux.bundle` from somewhere.
- [Two placeholder syntaxes coexist] (`{coder-session-id}` vs
  `{{bundle-session-id}}`) → No migration: renaming the existing variable would
  break every shipped and operator template. The spec documents both
  vocabularies side by side.

## Migration Plan

None required. Additive for all valid configurations; the breaking class is
limited to underscore-bearing placeholder literals that never rendered.
Rollback is a revert: no state, no schema, no persisted artifacts.

## Open Questions

1. Should `{{seat}}`, `{{bundle}}`, `{{harness}}` join this change so the
   motivating example loads end to end, or stay a follow-on? Recommendation:
   follow-on — `{{seat}}`'s value ("the Agentmux identity value") needs a
   definition this repository does not currently give, and `{{harness}}`
   overlaps the "harness command after `--`" ownership question in the task
   constraints.
2. The dispatch references a "runtime/coders README" that does not exist;
   the coders-adjacent docs read were `src/runtime/README.md`,
   `src/configuration/README.md`, and the maintainer guide's `coders.toml`
   section. Confirm no other doc was meant.
