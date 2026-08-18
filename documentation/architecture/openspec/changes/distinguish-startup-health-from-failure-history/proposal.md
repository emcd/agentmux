## Why

`bundle-lifecycle` describes bundle startup health in terms of past startup
attempts, and describes the startup-failure history without ever saying when a
record leaves it. Both readings are wrong about what the surfaces are for: health
answers "is this session serving now", and the failure history answers "why did
this session fail to start". Nothing in the spec ties a failure record to a
condition that can end, so an implementation that retains every record for the
life of the runtime directory satisfies the spec while reporting failures that no
longer apply — the specification sanctions a stale diagnostic rather than
forbidding it.

## What Changes

- Restate `startup_health` as a function of **current session readiness** rather
  than of recorded startup attempts. A session is ready when its transport
  reports it serving; the bundle is `healthy` when every configured session is
  ready and `degraded` when at least one is ready and at least one is not.
- **BREAKING (specification only):** the existing degraded condition, "at least
  one configured session is ready and at least one startup attempt failed", no
  longer holds. A session that is not ready contributes to `degraded` whether or
  not a startup attempt was ever recorded for it, and a recorded failure whose
  session is now serving does not.
- Give startup-failure records a defined lifetime: every record for a session is
  cleared when that session is next observed serving successfully. Bounded
  eviction at the history cap is retained and remains the only other way a record
  leaves.
- State that the startup-failure history is not an input to `startup_health`, so
  the two cannot be re-coupled by an implementation reading the log to decide
  health.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `bundle-lifecycle`: the Bundle Startup Health Model requirement is restated in
  terms of current readiness, and the startup-failure history requirement gains a
  record lifetime and an explicit statement that the history does not feed health.

## Impact

Specification only. The relay already derives `startup_health` from a per-list
readiness probe and already clears a session's failure records when it is next
observed serving, so this change closes a wording gap rather than altering
behavior.

`cli-surface` and `mcp-tool-surface` render `startup_health`,
`startup_failure_count`, and `recent_startup_failures`, and are deliberately
**not** modified: they describe payload shape, and restating the derivation or the
lifetime in each renderer is how the three drift apart.

Per-transport readiness rules stay in the transport contracts and are out of
scope here; this change says only that readiness is what the transport reports.
