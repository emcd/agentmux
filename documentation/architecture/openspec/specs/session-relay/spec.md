# session-relay Specification (hub)

## Purpose

The session-relay specification has been partitioned into 8 capability-scoped sibling specs (97 base requirements total: 94 relocated from the prior single-file spec at 44d59dd plus 3 ADDED by `add-pty-transport` archive at 774f116). The canonical normative content lives in those partition specs, not in this file. This file is the hub for navigation and archive-order safety:

- new requirements in the session-relay capability domain MUST land in the appropriate partition spec, not here;
- existing active OpenSpec changes whose deltas target a requirement now in a partition spec MUST relocate their delta spec file from `specs/session-relay/spec.md` to the appropriate `specs/<partition>/spec.md` before archiving;
- the MODIFIED/ADDED nature of the delta is irrelevant to the path-migration rule -- both kinds of delta must follow the requirement to its new partition.

The normative requirements previously governed: bundle membership configuration, reconciliation lifecycle and startup failure visibility, session routing primitives with canonical `session@bundle` identity, send/look/raww/choose operation contracts with their per-target response shapes, the ACP worker lifecycle and shared replay buffer (1000-entry cap with snapshot/cursor accessors), ACP permission/choice queue (bounded at `pending_max` default 256 with `choices.snapshot`/`requested`/`resolved` event carrier), policy vocabulary and authorization evaluation order, session type taxonomy (tmux/acp/ui/pubsub), persistent relay client streams, dynamic bundle file watching, and `SessionType` transport capability flags.

## Partitions

| Partition | Spec directory | Description |
|-----------|----------------|-------------|
| Addressing & Routing | `openspec/specs/addressing-routing/spec.md` | Canonical IDs, namespace semantics, target resolution, list payloads, raww target resolution |
| Delivery & Quiescence | `openspec/specs/delivery-quiescence/spec.md` | Send envelope, async queue lifecycle, terminal outcomes, ack semantics, and asynchronous terminal-outcome receipt |
| Transport Contracts | `openspec/specs/transport-contracts/spec.md` | Per-transport execution contracts (tmux, ACP, raww, Pty): worker lifecycles, transport capability flags, prime/wedge timeouts, copy-mode-transparent injection, and inter-transport error codes |
| Authorization & Scope | `openspec/specs/authorization-scope/spec.md` | Policy presets, authorization vocabulary and evaluation, scope controls, uniform cross-bundle auth, UI sender validation, and per-operation authorization mappings |
| Look & Stream Events | `openspec/specs/look-and-stream-events/spec.md` | Look operation (transport-agnostic + per-transport), persistent client streams, Hello registration, recipient routability, and stream event contracts |
| Choice Decisions | `openspec/specs/choice-decisions/spec.md` | Choice/decision envelope, queue lifecycle, operator classes |
| Bundle Lifecycle | `openspec/specs/bundle-lifecycle/spec.md` | Reconciliation, bundle up/down, startup health, file watching |
| Environment Variables | `openspec/specs/environment-variables/spec.md` | Coder/bundle/session environment variable precedence and container-injected overrides |

## Future capability (not yet a live sibling spec)

`runtime-api` is reserved for the embeddable runtime API capability owned by active change `embeddable-runtime-api`. When that change archives, `openspec/specs/runtime-api/spec.md` will be created with its 10 ADDED requirements (embeddable runtime boundary, public dispatch handler contract, identity descriptor separation, configurable embedded runtime roots, principal provisioning boundary, transport parity, content-type envelope discrimination, topology-independent relay semantics, ACK timeout cleanup). Until then, runtime-api does not appear as a live sibling.

## OpenSpec archive-order notes

The following requirements are targeted by in-flight change `deliver-async-terminal-outcomes` (relay/53, merged at `f2aea4e`, implementation-gated, not yet archived). Their delta spec file at `documentation/architecture/openspec/changes/deliver-async-terminal-outcomes/specs/session-relay/spec.md` has been split by this partition change into the resulting paths below; relay/53's archive will resolve them by requirement name:

- `Asynchronous Terminal-Outcome Receipt` (ADDED) -> `documentation/architecture/openspec/changes/deliver-async-terminal-outcomes/specs/delivery-quiescence/spec.md`
- `Async Delivery Observability` (MODIFIED) -> `documentation/architecture/openspec/changes/deliver-async-terminal-outcomes/specs/delivery-quiescence/spec.md`
- `Tmux Prime Timeout` (MODIFIED) -> `documentation/architecture/openspec/changes/deliver-async-terminal-outcomes/specs/transport-contracts/spec.md`

OpenSpec's `opsx-sync` resolves MODIFIED deltas and ADDS ADDED requirements by requirement name; the delta's `### Requirement:` text is portable across paths, only the containing `specs/<capability>/spec.md` directory changes. All three relay/53 targets now live in their per-partition delta spec paths.

Six active OpenSpec changes total previously referenced `session-relay`. The original seven-change set (`add-container-sandboxing`, `add-do-action-tool`, `add-pty-transport`, `add-about-surface-and-description-fields`, `deliver-async-terminal-outcomes` (relay/53), `embeddable-runtime-api`, `add-e2e-test-harness`) was reduced to six when `add-pty-transport` archived at 774f116 -- its delta spec landed in `archive/2026-07-15-add-pty-transport/` and its 3 live ADDED requirements (Pty Prime Timeout, Pty Wedged State Detection, Pty Default Per-Coder Dimensions) are now part of the 97-requirement session-relay base. The `partition-session-relay-spec` change migrates each active delta directory atomically; change owners rebase from master before archive to pick up the migrated paths. See `agentmux:todos/general/31` for the per-change mapping.

## Requirements

### Requirement: Session-Relay Specification Partition Index

The session-relay specification SHALL be the hub reference for the 8 partition specs listed in `## Partitions` above. All normative content for session-relay capability domains (bundle membership, reconciliation, routing, delivery, transport, authorization, look, stream events, choice, environment variables) SHALL be authored in the partition spec that matches the capability, not in this hub file.

Active OpenSpec changes SHALL migrate their delta spec files to the partition directory matching each requirement target before archive. Both `## MODIFIED Requirements` and `## ADDED Requirements` deltas follow this rule; the rule covers relocations of existing requirements and (via the future-capability note above) brand-new capabilities whose partition will be created on the active change's archive.

#### Scenario: Partition reference resolves to a sibling spec

- **WHEN** a reviewer looks up a session-relay requirement by name
- **THEN** the requirement text is found in the partition spec identified by the `## Partitions` table, not in this hub file

#### Scenario: Active change delta spec path migration

- **WHEN** an active OpenSpec change has a `MODIFIED Requirements` or `## ADDED Requirements` delta targeting a requirement now in a partition spec
- **THEN** that change's delta spec path SHALL be moved from `<change>/specs/session-relay/spec.md` to `<change>/specs/<partition>/spec.md` before archive
