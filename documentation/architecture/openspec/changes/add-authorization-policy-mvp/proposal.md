# Change: Add Session-Attached Authorization Policies with Reusable Presets (MVP)

## Why

Current relay/CLI/MCP flows validate request shape, but they do not yet define
one converged authorization model for capability access. We need policy
controls attached to sessions, reusable policy presets, one centralized
policy-decision point, and one denial schema so behavior does not drift across
surfaces as `look`, `send`, `do`, and future `find` expand.

## What Changes

- Add reusable policy preset source at:
  - `<config-root>/policies.toml`
- Add optional preset-default selector in `policies.toml`:
  - `default = "<policy-id>"`
  - if omitted, runtime uses a built-in conservative default policy
- Add session-level policy binding in bundle session definitions:
  - `policy = "<policy-id>"`
- Lock fail-fast loading behavior:
  - missing/invalid `policies.toml` is a startup/runtime error
  - unknown `policy` reference in a session is a validation error
  - no implicit authorization fallback in MVP
- Lock canonical control vocabulary and scopes:
  - `find`: `self` | `all:home` | `all:all`
  - `list`: `all:home` | `all:all`
  - `look`: `self` | `all:home` | `all:all`
  - `send`: `all:home` | `all:all`
  - `do`: map of `action_id -> (none | self | all:home | all:all)`
    - missing action entry defaults to `none`
- Lock authorization decision ownership:
  - relay is centralized policy evaluator
  - MCP/CLI are request validators/adapters only
  - MCP/CLI perform no shadow authorization validation/decisioning
- Lock validation-first execution ordering:
  1. request validation
  2. requester identity resolution
  3. bundle/target/action resolution
  4. authorization evaluation
  5. execution
- Lock denial contract for `authorization_forbidden` details:
  - required: `capability`, `requester_session`, `bundle_name`, `reason`
  - optional: `target_session`, `targets`, `policy_rule_id`
- Lock MVP posture:
  - `look` default self-only
  - cross-bundle `look` currently unsupported by runtime contract
  - default `send` scope remains `all:home` (cross-bundle requires explicit
    `all:all` policy scope)
- Add explicit `list` deny semantics:
  - deny returns `authorization_forbidden` (not empty success payload)

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Affected code (expected implementation scope):
  - policy loading/validation (`policies.toml` + session policy references)
  - relay authorization gates (`list`, `send`, `look`, later `do`/`find`)
  - MCP/CLI propagation of relay authorization outcomes
  - tests for deny/allow and validation-first precedence
