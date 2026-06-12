# Change: Add structured details to startup failure diagnostics

## Why

The 2026-06-11 production outage was undiagnosable from the host's own
output: the bundle watcher's `relay.bundle.load_failed` inscription flattened
the relay error to its message string (dropping the offending control and
value), and a host startup failure exited with only the aggregate
`failed to start relay for N bundle(s)` — no per-bundle reason reached the
journal or inscriptions. Tracked as issues/relay/37.

## What Changes

- `relay.bundle.load_failed` inscriptions carry the relay error's `code` and
  structured `details` alongside the human-readable reason.
- Before a host startup failure exits the process, every failed bundle emits
  a per-bundle reason to stderr and a `relay.bundle.startup_failed`
  inscription, both carrying the structured error details; the startup
  summary inscription is also emitted on this path.
- The relay host startup summary payload gains a nullable `details` field on
  each per-bundle entry, preserving the structured details a relay-layer
  failure carries.
- Policy scope rejection messages say "unknown scope value" (the only
  remaining rejection cause post cap-removal) and the error details list the
  expected scope ladder.

## Impact

- Affected specs: `cli-surface` (modified: Relay Host Startup Summary
  Contract per-bundle entry fields)
- Affected code: `src/relay/watcher.rs`, `src/commands/host/relay.rs`,
  `src/commands/host/summary.rs`, `src/commands/mod.rs`,
  `src/relay/authorization/loading.rs`
- Additive payload changes only; no consumer-visible field is removed or
  renamed.
