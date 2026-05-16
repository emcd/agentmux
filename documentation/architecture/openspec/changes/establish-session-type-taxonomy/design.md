## Context

The relay currently distinguishes client behavior using `client_class` in the
`hello` frame: `agent` sessions receive prompt injection; `ui` sessions receive
stream push events; `operator` sessions may decide permissions. This model has
two problems:

1. **Authorization conflation** — `Operator` is a third class whose only
   purpose is to gate permission decisions. That gate is already redundantly
   expressed by the `authorize_grant` policy capability. Two mechanisms for one
   control is one too many.
2. **Embeddability gap** — Embedded in-process agents receive envelopes as
   prompts and make direct tool calls — no tmux pane, no ACP channel, no
   stream socket. None of the existing classes describes this transport.
   Transport behavior must be a config declaration, not a connect-time
   assertion.

## Goals / Non-Goals

- **Goals**:
  - Replace hello `client_class` with session-type config subtable
    (`{tmux, acp, ui, pubsub}`).
  - Establish `session@bundle` as the canonical identity everywhere on the wire
    and in internal state.
  - Gate permission decisions on `authorize_grant` alone.
  - Recognize all four session types from day one; make unimplemented types
    fail fast without a parse error.
  - Rename `tui.toml` → `users.toml` (global users, not TUI-specific).
- **Non-Goals**:
  - Implement `ui` or `pubsub` delivery fully (fail-fast NYI is sufficient).
  - Introduce cross-bundle routing.
  - Define the extension protocol (a separate, later proposal).

## Decisions

### Session type as config declaration

Session type is the single subtable present on a `[[sessions]]` entry:
`[sessions.tmux]`, `[sessions.acp]`, `[sessions.ui]`, or
`[sessions.pubsub]`. Exactly one subtable is required; multiple or zero are
config errors.

**Why**: Transport behavior is fixed at operator configuration time. Asserting
it dynamically at connect time is redundant and spoofable.

**Alternatives considered**: Keep `client_class` in hello but cross-validate
against config. Rejected — dual sources of truth for the same fact; adds
parsing complexity for zero behavioral gain.

### Session-coder type consistency

`[sessions.tmux]` requires the referenced coder to have a `[coders.tmux]`
descriptor; `[sessions.acp]` requires `[coders.acp]`. Mismatch is a fail-fast
config error. `ui` and `pubsub` sessions carry no coder reference.

**Why**: A tmux session backed by an ACP coder or vice versa is an operator
misconfiguration that will manifest at runtime as a crash or silent
misbehavior. Catching it at load time is strictly better.

### Canonical identity: `session@bundle`

All relay state and wire output uses `{session_id}@{bundle_name}`.
Hydration happens at `hello` registration using `bundle_name` from the hello
frame. No additional round-trip required.

Global users (from `users.toml`) identify themselves with `session_id`
carrying a `@GLOBAL` suffix (e.g., `"user@GLOBAL"`). The relay recognizes the
suffix during hello lookup and searches `users.toml` rather than the bundle
member list.

**Why**: Bare session ids are ambiguous once cross-bundle routing or global
users are in scope. Establishing canonical form now avoids a second breaking
wire change later. The `@GLOBAL` suffix makes global-vs-local unambiguous with
no extra field.

**Wire breaking change**: Clients comparing against bare `"master"` must
compare against `"master@agentmux"`. Accepted because this is pre-MVP.

### Permission decisioning: `authorize_grant` alone

`UI-Mediated Decision Submitter Gate` is deleted. Any principal with
`authorize_grant` capability may submit `permission.resolve`, regardless of
session type.

**Why**: `Operator` class was the only mechanism enforcing this gate. Deleting
the class means the gate must move to the existing capability system, where it
already lives.

### `ui`/`pubsub` fail-fast NYI

Both types are parsed and validated from day one. At startup, any session with
type `ui` or `pubsub` emits a structured bootstrap failure
(`runtime_session_type_not_implemented`) rather than a parse error or silent
skip.

**Why**: Prevents operator confusion when the config is correct but the
implementation has not landed. Avoids changing the discriminator when the
implementation ships.

## Risks / Trade-offs

- **Test fixture churn** — ~20 test files, ~80 inline TOML sites reference the
  old `[[sessions]]` flat schema. Build a shared config-builder test helper
  first to contain the churn.
- **`tui.toml` rename** — any operator config referencing the old path breaks
  silently (missing file is not an error). Mitigated by clear release notes and
  the `data/configuration/` example rename.
- **Wire break** — `session@bundle` form and hello field removal are breaking
  changes. All in-process clients (MCP, TUI) are updated in the same batch;
  external clients do not exist pre-MVP.

## Migration Plan

1. Archive `add-mcp-permission-decision-surface`.
2. Implement config layer (tasks 1.x) — fail fast on old flat `[[sessions]]`
   schema in all bundles.
3. Implement relay hello + identity (tasks 2.x).
4. Implement routing + delivery by session type (tasks 3.x).
5. Implement permission gate removal (tasks 4.x).
6. Update MCP and TUI call sites (tasks 5.x).
7. Clean up tests (tasks 6.x).
8. Update data examples and docs (task 7.x).

## Future Considerations

### Mailbox delivery for `ui` and `pubsub`

A store-and-forward / cursor-advance model is a plausible future option for
`ui` and `pubsub` sessions: the relay buffers messages, notifies the client
that messages are waiting, and the client advances an explicit read cursor on
acknowledgement. This is not in scope here — fail-fast NYI is sufficient for
this proposal.

**Implementation posture**: when landing `ui` and `pubsub` delivery (a future
proposal), represent the delivery endpoint as a trait rather than a concrete
struct so a buffering implementation can be added without changing the session
type system or config schema.

### Tokio async migration sequencing

The Tokio async I/O migration (v0.5.0 Phase B) and `ui`/`pubsub` delivery
implementation (tasks 3.x here, plus a follow-on delivery proposal) overlap
at the async spawn and channel layer. **Sequence Tokio cutover before or
alongside `ui`/`pubsub` delivery implementation** to avoid two rounds of
delivery-path churn. Do not land delivery for these session types in the
pre-Tokio threading model if the migration is already planned.

## Open Questions

None for this proposal. All design questions resolved across relay, mcp, tui,
and api lanes prior to formalization.
