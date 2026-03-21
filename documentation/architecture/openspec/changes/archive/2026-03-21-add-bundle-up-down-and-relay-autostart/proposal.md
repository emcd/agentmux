# Change: Add bundle up/down lifecycle commands and relay autostart mode

## Why

Operators currently need explicit bundle/group selectors to start hosting.
That is awkward for service-manager startup and brittle for boot scripts.

## What Changes

- Add explicit bundle lifecycle commands:
  - `agentmux up`
  - `agentmux down`
- Keep lifecycle concerns separated:
  - `host relay` controls relay process lifecycle
  - `up/down` control bundle hosting lifecycle on a running relay
- Simplify relay host command to process-oriented no-selector mode:
  - `agentmux host relay` starts relay process and hosts autostart-eligible
    bundles
  - `agentmux host relay --no-autostart` starts process without hosting bundles
- Add bundle config eligibility flag:
  - optional top-level `autostart = true|false` in bundle TOML (default false)
- Keep bundle groups for `up/down` selectors only; remove group and bundle
  selectors from `host relay`.
- Define deterministic machine payloads and idempotent transition behavior for
  `up/down`.

## Impact

- Affected specs:
  - `cli-surface`
  - `runtime-bootstrap`
  - `session-relay`
- Affected code (implementation follow-up):
  - CLI command routing/parser
  - relay runtime bundle host/unhost control operations
  - startup and lifecycle summary payload rendering
