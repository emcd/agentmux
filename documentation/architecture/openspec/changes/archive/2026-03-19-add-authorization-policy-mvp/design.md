## Context

Authorization semantics currently live as implicit behavior spread across relay
runtime checks and surface-level request contracts. This creates drift risk as
new capabilities (`do`, `find`) and cross-bundle flows expand. MVP needs one
central policy decision point and one denial contract with validation-first
precedence.

## Goals

- Define reusable policy presets in one artifact.
- Bind policy to sessions directly in bundle configuration.
- Define one centralized authorization decision point in relay.
- Define one denial details schema for `authorization_forbidden`.
- Preserve validation-first ordering across relay/CLI/MCP surfaces.

## Non-Goals

- Implementing durable policy history/audit trails.
- Defining full persistent `find` storage/query semantics (tracked separately).
- Adding client-side authorization engines in MCP/CLI.

## Locked Decisions

### Policy Shape and Source

- Reusable presets live in:
  - `<config-root>/policies.toml`
- Optional top-level default selector MAY be provided:
  - `default = "<policy-id>"`
- When `default` is absent, runtime SHALL use a built-in conservative default
  policy for sessions that do not provide an explicit `policy` field:
  - `find = "self"`
  - `list = "all:home"`
  - `look = "self"`
  - `send = "all:home"`
  - `do` map defaults all actions to `none`
- Presets are declared as `[[policies]]` with:
  - `id` (required)
  - `description` (optional)
  - `[controls]` (required)
- Each session binds policy by id in bundle config:
  - optional `policy = "<policy-id>"`
- Resolution precedence for each session:
  1. explicit session `policy` when present
  2. top-level `default` preset when present
  3. built-in conservative default policy
- Missing/invalid policy artifact or unknown session policy id is fail-fast.

### Control Vocabulary and Scope Values

Controls and allowed scope values:

- `find`: `self` | `all:home` | `all:all`
- `list`: `all:home` | `all:all`
- `look`: `self` | `all:home` | `all:all`
- `send`: `all:home` | `all:all`
- `do`: map `action_id -> (none | self | all:home | all:all)`

Interpretation:

- `self` = requester session only
- `all:home` = any session in requester's associated bundle
- `all:all` = any session across bundles (subject to runtime support/trust)
- `none` = action/operation explicitly not allowed

For `do` controls:

- missing action-id entry defaults to `none`
- `all:home` and `all:all` are reserved/non-operative for current self-target
  `do` MVP behavior until non-self target selection is introduced

### Centralized Authorization Decisioning

- Relay is the only authorization decision point.
- MCP/CLI perform no shadow authorization validation/decisioning.
- MCP/CLI only validate request shape and adapt relay responses.

### Principal and Trust Boundary

- Authorization principal is association/socket-driven requester identity from
  runtime context.
- Caller-supplied sender-like fields are non-authoritative for principal
  identity.
- MVP trust boundary remains same-host/same-user local socket model.

### Evaluation Order (Validation-First)

1. Validate request structure and bounds.
2. Resolve requester association/principal.
3. Resolve bundle/target/action existence and unsupported scopes.
4. Evaluate authorization policy controls.
5. Execute runtime operation.

Validation errors win before policy denials.

### Denial Contract

When request is valid/resolved but denied by policy, relay returns:

- `code = authorization_forbidden`
- `details` minimum schema:
  - required: `capability`, `requester_session`, `bundle_name`, `reason`
  - optional: `target_session`, `targets`, `policy_rule_id`

### MVP Posture Locks

- `look` default remains self-only.
- Cross-bundle `look` remains unsupported by current runtime contract
  (`validation_cross_bundle_unsupported`) even if policy control is broader.
- Default `send` scope is `all:home`; cross-bundle send requires explicit
  policy scope `all:all`.

## Policy Model Shape (MVP)

Illustrative schema:

```toml
format-version = 1

[[policies]]
id = "default"
description = "Default local policy"

[policies.controls]
find = "self"
list = "all:home"
look = "self"
send = "all:home"

[policies.controls.do]
"compact" = "self"
"skill:deploy" = "none"
"status" = "all:home"
```

Session binding in bundle config:

```toml
[[sessions]]
id = "master"
name = "GPT (Coordinator)"
directory = "/home/me/src/WORKTREES/agentmux/master"
coder = "codex"
policy = "default"
```

## Risks and Trade-Offs

- Fail-fast policy loading may block startup if operators forget to provision
  `policies.toml`.
- `all:all` can express broader intent before full cross-bundle trust topology
  is implemented; runtime may still reject unsupported operations.
- Centralized relay authorization reduces drift risk but increases relay
  responsibility and test surface.

## Follow-Up Hooks

- `do` and `find` proposals should reuse this control vocabulary and denial
  schema without redefining either.
- Later trust-topology proposals can refine how `all:all` is constrained across
  bundles/relays without changing client contracts.
