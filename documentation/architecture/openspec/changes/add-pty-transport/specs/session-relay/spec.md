## MODIFIED Requirements

### Requirement: Bundle Membership Configuration

The system SHALL let operators define bundle membership in per-bundle TOML
files with kebab-case keys:

- `bundles/<bundle-id>.toml`

Each bundle file SHALL include:

- `format-version` (supported value for this schema: `1`)
- `[[sessions]]` entries with:
  - `id`
  - optional `name` (human-readable recipient name)
  - `directory`
  - exactly one session shape: a coder-backed shape (a flat `coder` reference,
    with optional `coder-session-id`) or a coder-less shape (exactly one
    `[sessions.ui]` or `[sessions.pubsub]` marker subtable)

Session membership invariants SHALL remain enforced:

- session `id` values are unique within one bundle
- optional session `name` values are unique within one bundle when present

A coder-backed `[[sessions]]` entry SHALL carry:

- required `coder` reference (must resolve to a `[[coders]]` entry)
- optional `coder-session-id`

The session's transport SHALL be derived from the referenced coder's
descriptor:

- `[coders.tmux]` → Tmux-backed coder delivery (existing)
- `[coders.acp]` → ACP-backed coder delivery (existing)
- `[coders.pty]` → Pty-backed coder delivery via libghostty-vt + portable-pty
  (new in `add-pty-transport`)

The session entry SHALL NOT restate the transport; the coder descriptor is
authoritative.

A coder-less `[[sessions]]` entry SHALL declare exactly one `[sessions.ui]` or
`[sessions.pubsub]` marker subtable, which SHALL carry no required fields
(empty body is valid). A coder-less entry SHALL NOT carry a `coder` or
`coder-session-id` field.

Coder definitions SHALL include target descriptors in `coders.toml`:

- `format-version` (supported value for this schema: `1`)
- `[[coders]]` entries with:
  - `id`
  - exactly one target descriptor table:
    - `[coders.tmux]`
    - `[coders.acp]`
    - `[coders.pty]`

Descriptor fields SHALL be:

- `[coders.tmux]`:
  - required `initial-command`
  - required `resume-command`
  - optional `prompt-regex`
  - optional `prompt-inspect-lines`
  - optional `prompt-idle-column`
- `[coders.acp]`:
  - required `channel` (`stdio` | `http`)
  - for `channel = "stdio"`: required `command`
  - for `channel = "http"`: required `url`; optional `headers` entries
    (`name`, `value`)
- `[coders.pty]` (new in `add-pty-transport`):
  - required `initial-command`
  - required `resume-command`
  - optional `prompt-regex`
  - optional `prompt-inspect-lines`
  - optional `prompt-idle-column`
  - optional `cols` (default 120) and `rows` (default 40)
  - optional `prime-timeout-ms`
  - optional `wedge-detection` (default `true`)

ACP lifecycle selection constraints:

- if ACP-backed session includes `coder-session-id`, runtime SHALL call
  `session/load` for that session.
- if ACP-backed session omits `coder-session-id`, runtime SHALL call
  `session/new` for that session.
- if ACP `session/load` fails, runtime SHALL fail that session and SHALL NOT
  silently fall back to `session/new`.

Pty and Tmux lifecycle selection constraints:

- if a coder-backed session includes `coder-session-id` and the coder defines
  `[coders.pty]` (Pty) or `[coders.tmux]` (Tmux), the runtime SHALL construct
  the resume command by substituting `{coder-session-id}` into the
  `resume-command` template.
- if the coder-backed session omits `coder-session-id`, the runtime SHALL
  use the `initial-command` template.
- if the template substitution leaves an unresolved placeholder, the
  validator SHALL reject the configuration during load.

Routing and delivery SHALL use session `id` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

#### Scenario: Load valid tmux-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference
- **AND** the referenced coder defines `[coders.tmux]`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the tmux transport

#### Scenario: Load valid ACP-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference with
  `coder-session-id`
- **AND** the referenced coder defines `[coders.acp]` with `channel = "stdio"`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the ACP transport

#### Scenario: Load valid Pty-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference
- **AND** the referenced coder defines `[coders.pty]` with
  `initial-command` and `resume-command`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the Pty transport
- **AND** the Pty transport spawns the child under a portable-pty master
  sized to the per-coder `cols` and `rows` (defaults 120 x 40)

#### Scenario: Reject session with neither coder nor marker

- **WHEN** a bundle session entry declares no `coder` reference and no
  `[sessions.ui]` or `[sessions.pubsub]` marker subtable
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Reject session declaring both coder and marker

- **WHEN** a bundle session entry declares a `coder` reference and also a
  `[sessions.ui]` or `[sessions.pubsub]` marker subtable
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Reject coder declaring both Pty and Tmux target descriptors

- **WHEN** a `[[coders]]` entry declares both `[coders.pty]` and
  `[coders.tmux]` subtables
- **THEN** the validator rejects the configuration with a structured config
  error

### Requirement: Session Routing Primitive

The system SHALL expose session ids as the routing primitive for message
delivery.
The system SHALL resolve each target session to its delivery endpoint at
delivery time using session type from config:

- `tmux` sessions: prompt-injection/quiescence delivery path
- `acp` sessions: ACP worker delivery path
- `pty` sessions: native PTY delivery path via libghostty-vt + portable-pty
  (new in `add-pty-transport`)
- `ui` sessions: stream push event delivery path
- `pubsub` sessions: embedded callback delivery path

The system SHALL support directed delivery to one or more explicitly selected
target sessions.

For send explicit targets, relay SHALL accept only canonical target identifiers
in `session@bundle` form or bare `session_id` values that resolve unambiguously
within the sending bundle.

Relay SHALL NOT resolve configured bundle session `name` values as send-target
aliases.
Session `name` remains informational metadata only and is not send-routable.

#### Scenario: Resolve session target for direct send by session type

- **WHEN** a caller sends a message to target `session_id`
- **THEN** the system routes to that session using its configured session type
- **AND** resolves the appropriate delivery endpoint for that type
- **AND** the resolution distinguishes `tmux`, `acp`, `pty`, `ui`, and `pubsub`
  delivery endpoints

#### Scenario: Reject configured name alias as explicit send target

- **WHEN** a caller sends a message to a configured `name` alias rather than
  the canonical `session_id`
- **THEN** the relay rejects the request with a validation error

### Requirement: Prompt-Readiness Template Gating

The system SHALL support optional per-member prompt-readiness templates that
must match before relay injection.

A prompt-readiness template SHALL support:

- `prompt_regex` (required)
- `inspect_lines` (optional, defaults to a bounded tail window)
- `input_idle_cursor_column` (optional)

`prompt_regex` SHALL be evaluated against a multiline string built from the
inspected non-empty tail lines of pane output. The "pane output" source is
transport-specific: Tmux reads from `capture-pane`; Pty reads from
`Formatter::format_alloc(Format::Plain)` via the `PtyOutputView` look path.

When `input_idle_cursor_column` is configured, relay SHALL treat the target as
prompt-ready only when the transport reports the cursor at that configured
column. For Tmux, this is `tmux display-message -p`; for Pty, this is
`Terminal::cursor_x()`.

Wedge detection defaults to enabled for both Tmux-backed and Pty-
backed sessions (the operator MAY opt out per coder via
`[coders.<id>.{tmux,pty}].wedge-detection = false`). When wedge
detection is enabled and the pane settles at a non-prompt-ready
state, the coder transport SHALL classify the flush group as
`wedged` rather than waiting indefinitely. The wedge detection knob
is independent of the prompt-readiness template configuration.

The wedge classifier is the same `Wedged` outcome for both Tmux
and Pty: `SendOutcome::Failed` + `reason_code = "pane_wedged"`
after `WEDGE_CONSECUTIVE_TICKS` (3) identical wedge-class
evaluations, OR when the prime window has elapsed with a wedge-
class mismatch observed. Per-transport knobs and Pty-specific
wedge scenarios live under the `Pty Wedged State Detection`
requirement; per-transport knobs live under the cross-cutting
`Pty Prime Timeout` requirement.

> **Re-scoped 2026-07-15 against the post-`remove-operator-
> interaction-delivery-gate` archive (master `2708884`).** The
> prior draft included a per-transport "Operator-interaction
> semantics differ between transports" subsection + three
> `operator_interaction_active`-conditional scenarios (Tmux
> silence, Pty always-false, Pty-doesn't-consult). All three
> are obsolete after the upstream copy-mode gate was retired
> (issues/relay/52). Pty wedge scenarios moved to the `Pty
> Wedged State Detection` ADDED requirement.

#### Scenario: Deliver when prompt-readiness template matches

- **WHEN** target member has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches the inspected multiline tail text
- **THEN** relay injects the message

#### Scenario: Match prompt plus status with one multiline regex

- **WHEN** target member uses one regex that spans prompt and status lines
- **AND** pane output tail contains those lines in order
- **THEN** relay treats target as prompt-ready

#### Scenario: Require idle input column before injection

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position equals configured
  `input_idle_cursor_column`
- **THEN** relay injects the message

#### Scenario: Do not inject while user is typing

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position differs from configured
  `input_idle_cursor_column`
- **THEN** relay does not inject the message
- **AND** relay continues waiting until wedge detection fires (when
  enabled), prime timeout fires (when enabled), or relay shuts down

#### Scenario: Time out when quiescent pane never becomes prompt-ready

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.{tmux,pty}].prime-timeout-ms` is set to a
  finite millisecond value
- **AND** pane output never begins flowing within the prime window
- **THEN** the transport resolves the flush group as
  `SendOutcome::Timeout`
- **AND** relay does not inject the message

#### Scenario: Classify as wedged when settled pane is not prompt-ready (default-on)

- **WHEN** target member has a prompt-readiness template
- **AND** the coder defines `[coders.<id>.tmux]` or
  `[coders.<id>.pty]` with `wedge-detection` not disabled (it
  defaults to enabled)
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **THEN** the coder transport resolves the flush group as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** relay does not inject the message

#### Scenario: Deliver to a pane the operator has scrolled into copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the
  operator scrolled it with the mouse wheel)
- **AND** the pane's live content is prompt-ready
- **THEN** relay injects the message
- **AND** the pane remains in copy-mode with the operator's scroll
  position undisturbed

#### Scenario: Wedge detection opt-out preserves prior behavior

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.{tmux,pty}].wedge-detection = false`
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **THEN** relay continues waiting until the pane becomes
  prompt-ready, prime timeout fires (if enabled), or relay shuts
  down

### Requirement: Transport Capability Contract

Every target reachable via look or raww SHALL have four transport capabilities,
derived at check time from its unified registry entry's `SessionType` rather than
stored as fields on the entry:

- `can_be_looked` — the session can be targeted by `look` (its transport
  supports snapshot capture)
- `can_be_written` — the session can be targeted by `raww` (its transport
  supports raw input injection)
- `can_stream_output` — the session's transport natively produces live
  output chunks (ACP and Pty stream output natively; Tmux requires periodic
  polling)
- `can_give_choices` — the session's transport can surface choice requests
  (the transport produces ACP-style option arrays for operator/UI resolution).
  Describes choice *production*, not resolution authority — any session with
  sufficient `choose` policy scope may resolve choices regardless of its own
  `can_give_choices` value.

Capabilities SHALL be derived from the entry's `SessionType` (at check time).
Bundle entries derive the type from bundle configuration at startup/reconcile;
relay-wide entries derive it from `users.toml` at startup for declared principals
(registered offline) or at Hello for dynamically-created principals. This makes
the registry entry the operation-time source of truth for target capabilities
instead of reloading different configuration sources for bundle and relay-wide
targets.

| Transport | `can_be_looked` | `can_be_written` | `can_stream_output` | `can_give_choices` |
|-----------|----------------|-----------------|--------------------|--------------------|
| `Tmux`    | true           | true            | false              | false              |
| `Acp`     | true           | true            | true               | true               |
| `Pty`     | true           | true            | true               | false              |
| `Ui`      | false          | false           | false              | false              |
| `Pubsub`  | false          | false           | false              | false              |

The `Pty` row is normative: Pty is a populated transport (per
`add-pty-transport`) and a coder-backed session whose coder defines
`[coders.<id>.pty]` derives `SessionType::Pty` with this capability row.
Bundle entries with `[coders.<id>.pty]` participate in `look` and `raww`
operations under the same capability checks as Tmux-backed entries.

`can_stream_output` is advertised on registration; streaming look semantics that
consume it are deferred to a follow-on proposal.

When a look or raww operation resolves a target whose entry-derived capability for
that operation is false, relay SHALL return `validation_unsupported_operation`.
This check precedes authorization policy checks and applies uniformly to bundle
targets and relay-wide targets.

#### Scenario: Reject look against session with can_be_looked false

- **WHEN** a `look` request resolves to a target whose `SessionType` derives
  `can_be_looked = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Reject raww against session with can_be_written false

- **WHEN** a `raww` request resolves to a target whose `SessionType` derives
  `can_be_written = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Permit look against session with can_be_looked true

- **WHEN** a `look` request resolves to a target whose `SessionType` derives
  `can_be_looked = true` (Tmux, ACP, or Pty)
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: Permit raww against session with can_be_written true

- **WHEN** a `raww` request resolves to a target whose `SessionType` derives
  `can_be_written = true` (Tmux, ACP, or Pty)
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: ACP session advertises can_give_choices true

- **WHEN** an ACP-backed session registers with the relay
- **THEN** its entry's `SessionType` derives `can_give_choices = true`

#### Scenario: Tmux session advertises can_give_choices false

- **WHEN** a Tmux-backed session registers with the relay
- **THEN** its entry's `SessionType` derives `can_give_choices = false`

#### Scenario: Pty session advertises can_give_choices false

- **WHEN** a Pty-backed session registers with the relay
- **THEN** its entry's `SessionType` derives `can_give_choices = false`

## ADDED Requirements

### Requirement: Pty Prime Timeout

The system SHALL surface a config-surfaced prime timeout knob for Pty-backed
sessions, applied as the `prime-timeout-ms` TOML key under the per-coder
`[coders.<id>.pty]` table (no `pty-` prefix; the table itself namespaces the
key). The knob SHALL bound the time the Pty transport waits, during the
quiescence wait for a flush group, for the target to produce observable output
before classifying the flush group as `unresponsive`. The knob is **opt-in**:
when absent or `None`, the Pty transport preserves the unbounded behavior
inherited from the shared wedge/prime state machine.

The Pty prime timeout SHALL be communicated from the relay to the Pty transport
through the same generic `DeliveryEnvelope.prime_timeout_ms: Option<u64>` field
introduced by `tmux-wedge-detection`. The relay populates this field from
`[coders.<id>.pty].prime-timeout-ms` at envelope construction time. The field
is generic across transports: the relay does not know which transport will
consume it; ACP's wedge-companion proposal populates the same field for ACP
sessions.

The prime timer semantics for Pty follow the merged `tmux-wedge-detection`
proposal:

- The prime timer SHALL start at the moment the Pty transport's internal
  delivery task begins the quiescence wait for a flush group.
- The prime timer SHALL NOT reset on coalesce-during-wait when new envelopes
  are absorbed into the flush group during the prime window.
- The prime timer SHALL fire when the prime window has elapsed with no
  observable output from the target, regardless of any rendering-state
  signal; Pty has no operator-interaction concept that suppresses
  classification (the upstream copy-mode gate was retired by
  `remove-operator-interaction-delivery-gate`, archived 2026-07-15).
  Copy-mode and other rendering states do not impede injection or
  affect what `cursor_x` and `capture-pane`-style probes report; the
  prime timer always measures observable output, not rendering state.

When the prime timer fires for a Pty target (no observable output within the
prime window), the Pty transport SHALL resolve every sender in the flush group
with `SendOutcome::Timeout`. The relay worker SHALL propagate that outcome to
the MCP/CLI caller as a distinct timeout result, not collapsed into `Failed`.

#### Scenario: Pty prime timeout fires on unresponsive target

- **WHEN** the bundle config sets `[coders.<id>.pty].prime-timeout-ms` to a
  finite millisecond value
- **AND** the Pty transport's internal delivery task begins the quiescence
  wait for a flush group
- **AND** the target produces no observable output before the prime window
  elapses
- **THEN** every sender in the flush group receives `SendOutcome::Timeout`
- **AND** no message is injected into the PTY

#### Scenario: Pty prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.pty].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Pty transport does not classify any flush group as
  `unresponsive`
- **AND** the only terminal failure modes for a flush group are `Failed` +
  `reason_code = "pane_wedged"` (when wedge detection is enabled, the
  default) and `Shutdown`

### Requirement: Pty Wedged State Detection

The system SHALL surface a config-surfaced wedge detection knob for Pty-backed
sessions, applied as the `wedge-detection` boolean TOML key under the per-coder
`[coders.<id>.pty]` table. The knob SHALL classify a settled, non-prompt-ready
pane as `wedged` via the shared wedge/prime state machine in
`src/transports/quiescence.rs`.

Wedge detection defaults to **enabled** (`true`) for Pty, matching the merged
`tmux-wedge-detection` rationale (cost of a silently-wedged pane is higher
than cost of a false-positive wedge). Operators MAY opt out by setting
`[coders.<id>.pty].wedge-detection = false`. The opt-out preserves the
unbounded-wait behavior.

A wedge detection SHALL fire when wedge detection is enabled and the Pty
transport observes, during the quiescence wait for a flush group:

- the pane output has been quiescent for at least one quiet window (probe
  `observe()` returns `is_prompt_ready = false` and the
  `activity_generation` field has not advanced since the previous
  observation)
- the prompt-readiness template does NOT match the inspected pane tail
  (formatter `format_alloc(Format::Plain)` tail text does not match
  `prompt_regex`)

When wedge detection fires, the Pty transport SHALL resolve every sender in
the flush group with `SendOutcome::Failed` and `reason_code = "pane_wedged"`.
The classification SHALL be sticky: once the flush group is classified as
wedged, the transport SHALL NOT re-evaluate across coalesce iterations.
Per-message wedge deadlines within a flush group are out of scope.

#### Scenario: Pty wedge fires on settled non-prompt-ready pane (default-on)

- **WHEN** the bundle config does not set
  `[coders.<id>.pty].wedge-detection` (or sets it to `true`)
- **AND** the Pty transport's quiescence wait observes the pane becomes
  quiescent
- **AND** the prompt-readiness template does not match the inspected pane
  tail (read via `Formatter::format_alloc(Format::Plain)`)
- **THEN** every sender in the flush group receives `SendOutcome::Failed`
  with `reason_code = "pane_wedged"`
- **AND** no message is injected into the PTY

#### Scenario: Pty wedge detection opt-out preserves unbounded behavior

- **WHEN** the bundle config sets `[coders.<id>.pty].wedge-detection = false`
- **THEN** the Pty transport continues to wait past quiescence until the
  pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are `Timeout`
  (if prime timeout is enabled and fires) and `Shutdown`

#### Scenario: Pty wedge is sticky across coalesce iterations

- **WHEN** the Pty transport's quiescence wait classifies a flush group as
  `wedged`
- **AND** new envelopes are absorbed into the flush group via
  coalesce-during-wait before the wedge classification propagates
- **THEN** every sender in the enlarged flush group receives the same wedge
  outcome (`Failed` + `reason_code = "pane_wedged"`)
- **AND** the transport does NOT re-evaluate wedge state across coalesce
  iterations

### Requirement: Pty Default Per-Coder Dimensions

The system SHALL provide per-coder default grid dimensions for Pty-backed
sessions, applied as the `cols` and `rows` TOML keys under the per-coder
`[coders.<id>.pty]` table. Both keys default to:

- `cols = 120`
- `rows = 40`

The Pty transport SHALL spawn the child under a `portable_pty` master sized to
these dimensions and SHALL call `Terminal::resize(cols, rows, 0, 0)` once at
startup. The transport SHALL read these values at startup only; runtime resize
(via a future `agentmux resize <session> <cols> <rows>` command) is out of
scope for `add-pty-transport` and deferred to a follow-up proposal.

`look()` SHALL return `LookSnapshotPayload::Lines { snapshot_lines }` from
`Formatter::format_alloc(Format::Plain)` truncated to the consumer's
`LookMode.lines`. The terminal's actual grid may be any size (post-resize or
post-reflow); the consumer asks for what it wants. There is no requirement
that the relay-tui consumer's viewport match the Pty-backed session's grid
dimensions; multi-viewer dimension reconciliation is out of scope for
`add-pty-transport` and deferred to a follow-up proposal.

#### Scenario: Pty spawns at per-coder default dims

- **WHEN** the bundle config does not set `[coders.<id>.pty].cols` or `.rows`
  (or sets them to the default values)
- **THEN** the Pty transport spawns the child under a 120 x 40 PTY master
- **AND** `Terminal::resize(120, 40, 0, 0)` is called once at startup

#### Scenario: Pty honors explicit per-coder dims

- **WHEN** the bundle config sets `[coders.<id>.pty].cols = 200` and
  `.rows = 60`
- **THEN** the Pty transport spawns the child under a 200 x 60 PTY master
- **AND** `Terminal::resize(200, 60, 0, 0)` is called once at startup

#### Scenario: Pty rejects zero-dimension config

- **WHEN** the bundle config sets `[coders.<id>.pty].cols = 0` or `.rows = 0`
- **THEN** the validator rejects the configuration with a structured config
  error during load