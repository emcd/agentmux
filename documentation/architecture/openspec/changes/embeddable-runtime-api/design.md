## Context

Identity federation is complete and archived as
`2026-06-03-add-identity-federation`, so the relay has stable principal
verification, introspection, revocation, and attribution contracts to build on.

The resolved embedding design is public-API-first: relay protocols are layered
over public handlers, not the other way around. The standalone relay binary is
the first consumer of the embeddable API. A foreign host such as `litrpg` uses
the same public runtime boundary in-process.

## Goals

- Make public dispatch, provisioning, and introspection functions the Rust
  embedding boundary.
- Ensure the standalone relay server, socket, MCP, CLI, stdio, and future
  transports call the same public handlers.
- Preserve equivalent relay semantics across embedded, standalone, and sidecar
  topologies.
- Keep caller-supplied identity descriptors distinct from relay-verified
  principal context.
- Accept caller-configured config/state roots for embedded runtime startup.
- Support `direct_psk` for host-held in-memory credential material.
- Add `Content-Type` as the canonical envelope payload discriminator.
- Specify ACK correlation cleanup so transport-accepted work cannot leave stale
  pending state when `accept_ack` never arrives.
- Provide a coherent delivery execution, observation, and lifecycle boundary
  for embedders, alternate hosts, diagnostics, and deterministic test harnesses.

## Non-Goals

- No extension registry or extension discovery surface in this proposal.
- No MCP poll surface for extension events in this proposal.
- No application-domain authorization requirements.
- No cross-relay trust or discovery topology.
- No public contract for private socket wire frames.
- No direct public contract for worker-registry entries, internal delivery-task
  fields, worker-close functions, or terminal-outcome completion functions.

## Decisions

- Decision: the embeddable boundary is public typed relay handlers and typed
  request/result structures, not a transport adapter trait.
- Decision: the standalone relay binary embeds the runtime by constructing the
  same public runtime object and calling the same handlers that foreign hosts
  call.
- Decision: transport adapters own framing, serialization, connection liveness,
  and stream lifecycle only.
- Decision: public dispatch takes relay-verified principal context. Caller
  identity descriptors and credentials are input to verification/provisioning,
  not authorization context.
- Decision: embedded runtime initialization accepts caller-configured config and
  state roots; Agentmux owns its internal layout beneath the configured state
  root and does not assume a fixed parent layout.
- Decision: the public identity descriptor credential-source vocabulary includes
  `direct_psk` so embedded agents can authenticate with host-held in-memory PSK
  material.
- Decision: private Hello/request/response frames remain socket protocol
  details.
- Decision: `Content-Type` discriminates envelope payload semantics while
  keeping `text/plain` behavior compatible with existing message delivery.
- Decision: content types that require `accept_ack` must have a bounded timeout
  and must clear pending correlation state when the timeout fires.
- Decision: the runtime owns delivery task construction, worker registration,
  receipt generation, correlation, and shutdown gating. Embedders may inject a
  transport-neutral delivery executor that receives resolved public delivery
  input and returns typed outcomes, and may subscribe to typed delivery lifecycle
  observations.
- Decision: the public runtime handle supports ordinary handler dispatch and a
  controlled shutdown lifecycle. Dispatch and observation remain at the runtime
  contract level; callers do not directly enqueue internal tasks, mutate the
  worker registry, close individual workers, or complete outcomes.

## Risks / Trade-offs

- Public handler boundaries may expose implementation seams that were previously
  private. Mitigate by publishing typed operation/context types rather than
  private frame structs.
- Refactoring the standalone relay to consume the new API may be larger than a
  foreign-host-only library wrapper. This is intentional: two relay semantics
  paths would drift.
- `Content-Type` support creates space for Proposal B but does not by itself
  define extension registration. Keep extension-specific behavior out until the
  follow-up proposal.
- An injectable executor can become a test hook disguised as API if it mirrors
  private worker machinery. Mitigate by exposing resolved delivery input and
  typed outcomes that are independently useful to alternate transports and
  hosts, while keeping registry and task invariants inside the runtime.

## Migration Plan

1. Introduce public runtime state/configuration types and public handler
   functions for existing relay operations.
2. Move standalone relay socket handling to frame/decode requests and call the
   public handlers.
3. Move MCP, CLI, and stdio surfaces to call public handlers directly or through
   thin clients that preserve the same handler contract.
4. Add public identity descriptor, verification, provisioning, verified
   principal context, root-configuration, and `direct_psk` credential-source
   types.
5. Add `Content-Type` to the canonical envelope model with `text/plain` as the
   default/current message behavior.
6. Add ACK pending-state timeout cleanup for content types that require
   `accept_ack`.
7. Add parity tests covering standalone socket use and in-process handler use.
8. Add a transport-neutral delivery executor, lifecycle observer, and controlled
   runtime shutdown contract without publishing worker-registry internals.
9. Add deterministic tests that drive delivery outcomes and shutdown races
   through that public runtime contract.

## Open Questions

- Exact public Rust type names are implementation details as long as the
  caller-supplied descriptor and relay-verified context remain distinct.
- Exact `accept_ack` timeout duration is an implementation/configuration detail,
  but timeout behavior and stale-correlation cleanup are normative.
