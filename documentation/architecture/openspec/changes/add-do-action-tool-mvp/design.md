## Context

`look` enables observation of session state but does not provide an equivalent
actuator for routine maintenance actions. We need a constrained action-dispatch
surface that can trigger known prompts safely, especially for self-target
operations where synchronous waiting can deadlock.

## Goals

- Provide a single action mechanism across CLI and MCP.
- Make action capabilities discoverable at runtime.
- Keep execution constrained to configured allowlisted actions.
- Avoid self-target deadlocks by forcing async behavior.

## Non-Goals

- Full dynamic `agentmux help` framework in MVP.
- Arbitrary prompt execution outside configured actions.
- Cross-bundle policy expansion in MVP.

## Decisions

- Decision: `do` command surface uses two primary forms:
  - `agentmux do` (list)
  - `agentmux do --show <action>` (show metadata)
  - `agentmux do <action>` (execute)
- Decision: query details are available through action-specific show mode
  (`do --show <action>` in CLI; `mode=show` in MCP), which can be
  reused by future generic `help` surfaces.
- Decision: MCP exposes one `do` tool with mode-based payload rather than
  multiple tools to keep dynamic action evolution stable.
- Decision: action configuration uses kebab-case TOML keys and is defined per
  coder in `coders.toml` under canonical path `[[coders.do-actions]]` so
  action prompts can differ by coder.
- Decision: action entries include `self-only` with default `true`.
- Decision: in MVP, `self-only` is forward-compat/reserved because non-self
  targeting is out of scope; runtime behavior is still self-target-only by
  contract.
- Decision: MVP run execution is self-target only and does not expose target
  selector fields in CLI/MCP/relay request contracts.
- Decision: self-target execution is always effective async; request-level sync
  preference does not override this rule.
- Decision: `do run` returns one canonical acceptance payload across
  relay/CLI/MCP with required fields: `schema_version`, `bundle_name`,
  `requester_session`, `action`, `status`, `outcome`, `message_id`.
- Decision: MVP enforces allowlist + `self-only` only; broader authorization
  constraints are deferred to the existing authorization track.
- Decision: action execution emits standard inscriptions for observability.

## Risks / Trade-offs

- Trade-off: mode-based MCP tool schema is slightly more complex than separate
  tools, but better supports dynamic action catalogs.
- Risk: action prompt templates can be coder-specific and drift over time.
  Mitigation: keep discovery/show output explicit and source-of-truth from
  configuration.

## Migration Plan

- Add `coders.toml` action parsing (kebab-case) with conservative defaults.
- Add relay dispatch operation and wire CLI/MCP adapters.
- Introduce tests before enabling non-self targets in a follow-up change.

## Follow-Ups

- Generic dynamic `agentmux help` subcommand/tool built on top of `do`
  show metadata.
- Optional parameterized action payloads beyond simple prompt templates.
- Reuse shared authorization scope evaluator from
  `add-authorization-policy-mvp` for `do.list` / `do.show` / `do.run`
  including implicit-missing-entry => `none` behavior.
