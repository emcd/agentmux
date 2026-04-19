# Change: Relock Bundle Startup Health and Failure Visibility

## Why
Bundle startup currently lacks a deterministic, cross-surface contract for
partial startup success and machine-readable startup failure visibility.
Operators need resilient hosting semantics (start what can start) while TUI/MCP
consumers need deterministic startup health and startup-failure evidence.

## What Changes
- Lock deterministic two-phase bundle startup evaluation:
  - preflight phase
  - full per-session startup pass (attempt all configured sessions)
- Keep process-level no-selector `agentmux host relay` startup semantics
  unchanged.
- Keep non-breaking bundle state shape (`state=up|down`) and add explicit
  degraded indicator (`startup_health`) when `state=up`.
- Add mandatory startup-failure visibility contract:
  - live per-session startup-failure event/inscription
  - bounded persisted per-bundle startup-failure history
  - required list payload fields for failure count/history
- Lock deterministic reason-code reuse and bundle down-state reason precedence.

## Impact
- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Affected code:
  - relay startup/reconcile lifecycle and list response serialization
  - runtime startup-failure persistence and retention bookkeeping
  - CLI/MCP list output and fallback synthesis behavior
