# Change: Harden relay hello identity ownership and stream delivery assurance

## Why

Recent runtime incidents showed two related failure modes on persistent relay
streams:

1. duplicate `hello` identity claims can silently rebind ownership and create
   temporary message black holes;
2. stream-delivery transitions are not machine-explicit enough to make
   undeliverable paths obvious to senders.

We need deterministic ownership and completion contracts before adding more
stream clients.

## What Changes

- Tighten relay `hello` ownership semantics:
  - one active owner per `(bundle_name, session_id)`;
  - reject duplicate live claims with `runtime_identity_claim_conflict`;
  - allow replacement only with hard-dead evidence in MVP.
- Update reconnect contract to align with ownership hardening and remove
  implicit latest-claim-wins behavior.
- Lock stream delivery-assurance phase semantics keyed by `message_id`:
  - acceptance remains immediate response semantics;
  - completion updates carry machine phase/outcome details.
- Preserve current disconnected-UI queue/retry behavior.
- Add explicit transport/recipient behavior matrix so stream-only rules do not
  bleed into tmux/ACP delivery paths.

## Impact

- Affected specs:
  - `session-relay`
  - `runtime-bootstrap`
- Affected code (implementation follow-up):
  - relay stream registry claim handling
  - stream completion/update event emission
  - reconnect handling in runtime clients
  - stream delivery tests (duplicate claim, stale binding, completion updates)
