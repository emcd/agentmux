## REMOVED Requirements

### Requirement: ACP Stop-Reason Outcome Mapping

**Reason**: No ACP stop reason maps to a delivery outcome any more, so the
requirement is false in whole rather than stale in one bullet.

An ACP delivery now resolves from typed submission evidence at the framed
`session/prompt` write: on a successful write every member of the group
resolves `Delivered` before the replay-buffer locks or `on_dispatched` run, and
the turn lifecycle that follows is observability only. A completion arriving
later cannot select a delivery outcome, because the outcome was already
terminal when the write succeeded.

Each of the four mappings fails independently of this change:

- the terminal stop reasons mapping to `delivered` assert a causal relationship
  that no longer exists — the outcome is `delivered` regardless of which stop
  reason arrives, and would be `delivered` if none ever did
- `cancelled` -> `failed` with `reason_code = acp_stop_cancelled` has no
  implementation at all; that reason code appears nowhere in the source
- the turn-timeout mapping named a bound and a reason code (`acp_turn_timeout`)
  that were both deleted along with the prime timer and readiness latch
- dropped-on-shutdown is a shutdown-lifecycle behavior rather than a stop-reason
  mapping, and was only ever housed here by category error

**Migration**: The surviving completion semantics belong to the `ACP Terminal
Readiness Tracking` requirement, which governs what a completion signals about
readiness and respawn. The shutdown mapping is carried normatively by the
`Relay Stream Event Contract` requirement in `look-and-stream-events`, which
this change also updates and which retains `dropped_on_shutdown` ->
`phase=failed`, `outcome=failed`, `reason_code=dropped_on_shutdown`. No
behavior is being removed here — only a description of behavior that the
delivery path stopped exhibiting.
