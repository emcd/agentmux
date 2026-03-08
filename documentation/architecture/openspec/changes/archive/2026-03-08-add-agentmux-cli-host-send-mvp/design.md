## Context

The project targets a unified operator-facing CLI topology under `agentmux`
for hosting and direct relay operations. The design scope includes migration to
one canonical executable surface for host/list/send flows.

## Goals

- Define a canonical command shape under `agentmux`.
- Choose command verbs that fit current naming preferences (`host`, `list`,
  `send`).
- Support common shell workflow for sending message bodies from pipelines.
- Complete migration to a single executable surface (`agentmux ...`).

## Non-Goals

- Multi-bundle single-process relay hosting (`host relay --all`)
- Bundle watch/reconcile mode (`host relay --watch`)
- Relay lifecycle stop/unhost command topology
- Interactive cat-like send input mode for TTY stdin in MVP

Tracked follow-up todos:
- `agentmux:todos/runtime/8` (`host relay --all`)
- `agentmux:todos/runtime/9` (`host relay --watch`)
- `agentmux:todos/runtime/10` (stop/unhost topology)
- `agentmux:todos/runtime/11` (interactive TTY send mode)

## Decisions

- Decision: Use `host` as the process-hosting verb.
  - Why: aligns with short Germanic-style verb preference and avoids mixing
    `serve`/`start` terminology.
- Decision: Require positional bundle argument for relay hosting.
  - Shape: `agentmux host relay <bundle-id>`
  - Why: expresses required identity directly and reduces flag noise in common
    startup commands.
- Decision: Keep `send` as the direct message command.
  - Why: clearer imperative user intent than `chat` for one-shot CLI use.
- Decision: Resolve `send` message content from one source in MVP:
  `--message` or piped stdin.
  - Why: supports shell composition while keeping MVP interaction model simple.
- Decision: Remove legacy wrapper binaries.
  - Why: one canonical executable reduces ambiguity, startup guidance drift,
    and maintenance overhead.

## Message Input Resolution

For `agentmux send`:

1. If `--message` is present, use it.
2. Else, if stdin is piped, read stdin as full message body.
3. Else, return a structured `validation_missing_message_input` error.

If both `--message` and piped stdin are provided, return structured
`validation_conflicting_message_input` error.

This proposal intentionally defers interactive TTY line-capture mode.

## Risks / Trade-offs

- Introducing a new primary command adds parser and routing complexity.
  - Mitigation: integration tests around canonical `agentmux` host/list/send
    surfaces.
- Positional `<bundle-id>` for relay may diverge from existing flag-based
  startup habits.
  - Mitigation: document migration and keep aliases (`--bundle-name`) on
    relevant host/list/send flags where applicable.

## Migration Plan

1. Introduce `agentmux` command with new subcommand topology.
2. Reuse existing runtime startup code paths to minimize behavior drift.
3. Remove wrapper binaries and legacy wrapper tests.
4. Update documentation to use canonical commands exclusively.

## Follow-Up Direction

MCP tool naming should align from `chat` to `send` in a follow-up change so
CLI and MCP surfaces use the same action verb. This is tracked in
`agentmux:todos/mcp/11`.
