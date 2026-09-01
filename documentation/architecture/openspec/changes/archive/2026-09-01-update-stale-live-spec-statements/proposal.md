## Why

Six statements in the live specifications contradict the implementation, a
sibling capability, or themselves. Each was verified against source or against a
second live spec, not inferred from wording.

Two of them are actively misleading rather than merely untidy. `addressing-routing`
asserts "exactly four session types" and omits Pty, so a reader checking
conformance against it concludes a shipped transport is non-conforming. And
`runtime-bootstrap`'s `[delivery]` key table omits `unreachable-dwell-ms`, so an
operator cannot discover a real configuration key from the schema that is
supposed to define it.

Now, because these are the residue of changes that have already landed. Each
grows harder to attribute the longer it sits, and two of them are load-bearing
for readers who have no other source of truth.

## What Changes

- **`runtime-bootstrap`** — add `unreachable-dwell-ms` to the `[delivery]` key
  table with its implemented default and range. The requirement already
  discusses the key by name while its own table omits it, and asserts a
  zero-exclusion rule over a table that does not cover it.
- **`addressing-routing`** — correct `Session Type Taxonomy` from four session
  types to five, adding the `Pty` row.
- **`look-and-stream-events`** — stop describing `take_replay_entries` as an
  existing accessor reserved for non-look consumers. It is removed.
- **`relay-routing-layer`** — stop describing `on_behalf_of` as reserved and
  deferred to a follow-on. That follow-on landed. Also drop the
  proposal-slice phrasing ("this slice") from the same requirement.
- **`tui-surface`** — repoint the stream-transport citation from the archived
  change `add-relay-stream-hello-transport-mvp` to the live capability that owns
  those contracts.
- **`cli-surface`** and **`authorization-scope`** — replace the `tui.toml`
  references with the files that exist: `users.toml` for session identity,
  `ui.toml` for UI-surface defaults.
- **`session-relay`** (prose only) — correct the four-session-type echo in the
  hub preamble so the taxonomy fix is not stranded in one of its two sites.

No behavior changes. No **BREAKING** changes. Every edit brings a specification
into agreement with what already ships.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `runtime-bootstrap`: `Relay Configuration File` — `[delivery]` table gains the
  missing key row.
- `addressing-routing`: `Session Type Taxonomy` — four types becomes five.
- `look-and-stream-events`: `ACP Look Snapshot Contract` — the removed draining
  accessor is no longer described as available.
- `relay-routing-layer`: `Cross-Relay Target Ingress Filter` — `on_behalf_of` is
  no longer described as reserved and deferred.
- `tui-surface`: `Initial TUI Workflow Coverage` — normative authority moves from
  an archived change to a live capability.
- `cli-surface`: `CLI raww actor identity resolution` — `tui.toml` becomes
  `users.toml`.
- `authorization-scope`: `UI Request-Path Sender Validation` — `tui.toml` becomes
  `users.toml`.

Two further sites are prose rather than requirements and are edited directly,
since no delta mechanism governs them: the `cli-surface` Purpose preamble and the
`session-relay` hub preamble.

## Impact

Specifications only. No source, tests, configuration schema, or public interface
is touched, and no delivered behavior changes.

The principal risk is internal to this change rather than to the system: a
`## MODIFIED Requirements` delta replaces the **entire** named requirement at
sync, so any scenario not carried forward is deleted from the live spec. Seven
requirements are modified here, several carrying many scenarios, and the whole
point of the change is that nothing behavioral moves. Verification that the
retained set is complete is therefore the substance of the work, not a formality.

Retires `todos/openspec/3` and `todos/general/34`.

Deliberately out of scope, tracked separately:
- the per-capability allowed-scope cap contradiction (`todos/openspec/4`), which
  removes a normative rule rather than correcting a stale statement;
- retiring `session-relay` as a capability (`todos/openspec/7`), which would
  delete a whole capability spec — the first such removal in this project.
