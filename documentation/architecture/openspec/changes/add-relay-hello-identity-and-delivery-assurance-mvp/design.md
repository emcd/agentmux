## Context

Persistent relay streams currently allow latest-claim-wins identity rebinding and
lack a fully deterministic stream-completion contract keyed by `message_id`.
This creates avoidable ambiguity for delivery troubleshooting and enables
identity hijack windows when duplicate clients race.

## Goals

- Enforce deterministic single-owner stream identity semantics in MVP.
- Preserve existing disconnected-UI queue/retry behavior.
- Provide a machine-consumable stream completion carrier that allows senders to
  detect undeliverable paths without guessing from logs.

## Non-Goals

- Introduce lease/TTL ownership semantics in this slice.
- Add active liveness probes in claim path for MVP.
- Change tmux/ACP transport delivery contracts.

## Decisions

1) Single active owner for `(bundle_name, session_id)`
- Duplicate live claims are rejected with `runtime_identity_claim_conflict`.
- Required conflict details remain identity-centric and stable.

2) Hard-dead evidence only in MVP
- Ownership replacement is allowed only after immediate hard-dead evidence
  (closed stream, read/write failure, or explicit disconnect already observed by
  relay).
- Claim-path probes are deferred to avoid unbounded waits and policy drift.

3) Explicit transport/recipient matrix
- Agent recipients keep existing prompt-injection/quiescence path and do not
  require active stream registration.
- UI recipients keep stream push + disconnected queue/retry semantics.
- Stream-only assurance rules are scoped to stream transport behavior and do not
  mutate tmux/ACP semantics.

4) Canonical completion carrier
- Stream completion updates are keyed by `message_id` and carry phase/outcome
  fields with deterministic payload shape.
- External terminal vocabulary remains unchanged (`success|timeout|failed`);
  `routed` remains diagnostic metadata, not a new terminal value.

## Risks and Trade-offs

- Without probes, stale-live false positives may require one extra reconnect
  cycle before ownership transfers.
- This trade-off is accepted for MVP to keep claim path deterministic and avoid
  probe-based contention.

## Follow-ups

- Forced takeover workflow with operator audit controls.
- Lease/TTL ownership model.
- Optional bounded liveness-probe algorithm as a later proposal.
