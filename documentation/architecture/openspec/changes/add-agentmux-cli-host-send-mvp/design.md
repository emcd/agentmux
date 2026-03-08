## Context

The project currently exposes service binaries (`agentmux-relay`,
`agentmux-mcp`) and MCP tool operations (`list`, `chat`), but not a unified
operator-facing CLI command topology. Recent team discussion favors short
Germanic verbs and a primary `agentmux` command.

This change introduces that topology without breaking existing harness
configurations that still invoke legacy binaries.

## Goals

- Define a canonical command shape under `agentmux`.
- Choose command verbs that fit current naming preferences (`host`, `list`,
  `send`).
- Support common shell workflow for sending message bodies from pipelines.
- Preserve backward compatibility for existing relay/MCP binary invocations.

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
- Decision: Preserve legacy binaries as wrappers.
  - Why: existing harnesses and configs can migrate gradually.

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
  - Mitigation: wrapper delegation and integration tests for parity.
- Positional `<bundle-id>` for relay may diverge from existing flag-based
  startup habits.
  - Mitigation: document migration and keep wrapper compatibility.

## Migration Plan

1. Introduce `agentmux` command with new subcommand topology.
2. Reuse existing runtime startup code paths to minimize behavior drift.
3. Keep `agentmux-relay` and `agentmux-mcp` wrappers delegating to shared
   execution logic.
4. Update documentation to prefer canonical commands.

## Follow-Up Direction

MCP tool naming should align from `chat` to `send` in a follow-up change so
CLI and MCP surfaces use the same action verb. This is tracked in
`agentmux:todos/mcp/11`.
