# session-relay Specification (hub)

## Purpose

The session-relay specification has been partitioned into 9 capability-scoped sibling specs. The canonical normative content lives in those partition specs, not in this file. This file is the hub for navigation and for the delta-path rule below.

The partition began with 97 requirements (94 relocated from the prior single-file spec at 44d59dd, plus 3 ADDED by the `add-pty-transport` archive at 774f116). That is a fact about where the partition started, not a count to keep current: the live total is whatever the nine specs hold, and

```
grep -h '^### Requirement' openspec/specs/{addressing-routing,delivery-quiescence,transport-contracts,authorization-scope,look-and-stream-events,choice-decisions,bundle-lifecycle,environment-variables,raww}/spec.md | wc -l
```

answers it without a number here going stale between archives.

- new requirements in the session-relay capability domain MUST land in the appropriate partition spec, not here;
- existing active OpenSpec changes whose deltas target a requirement now in a partition spec MUST relocate their delta spec file from `specs/session-relay/spec.md` to the appropriate `specs/<partition>/spec.md` before archiving;
- the MODIFIED/ADDED nature of the delta is irrelevant to the path-migration rule -- both kinds of delta must follow the requirement to its new partition.

The normative requirements previously governed: bundle membership configuration, reconciliation lifecycle and startup failure visibility, session routing primitives with canonical `session@bundle` identity, send/look/raww/choose operation contracts with their per-target response shapes, the ACP worker lifecycle and shared replay buffer (1000-entry cap with snapshot/cursor accessors), ACP permission/choice queue (bounded at `pending_max` default 256 with `choices.snapshot`/`requested`/`resolved` event carrier), policy vocabulary and authorization evaluation order, session type taxonomy (tmux/acp/pty/ui/pubsub), persistent relay client streams, dynamic bundle file watching, and `SessionType` transport capability flags.

## Partitions

| Partition | Spec directory | Description |
|-----------|----------------|-------------|
| Addressing & Routing | `openspec/specs/addressing-routing/spec.md` | Canonical IDs, namespace semantics, target resolution, list payloads |
| Delivery & Quiescence | `openspec/specs/delivery-quiescence/spec.md` | Send envelope, async queue lifecycle, terminal outcomes, ack semantics, and asynchronous terminal-outcome receipt |
| Transport Contracts | `openspec/specs/transport-contracts/spec.md` | Per-transport execution contracts (tmux, ACP, Pty): worker lifecycles, transport capability flags, copy-mode-transparent injection, and inter-transport error codes |
| Authorization & Scope | `openspec/specs/authorization-scope/spec.md` | Policy presets, authorization vocabulary and evaluation, scope controls, uniform cross-bundle auth, UI sender validation, and per-operation authorization mappings |
| Look & Stream Events | `openspec/specs/look-and-stream-events/spec.md` | Look operation (transport-agnostic + per-transport), persistent client streams, Hello registration, recipient routability, and stream event contracts |
| Choice Decisions | `openspec/specs/choice-decisions/spec.md` | Choice/decision envelope, queue lifecycle, operator classes |
| Bundle Lifecycle | `openspec/specs/bundle-lifecycle/spec.md` | Reconciliation, bundle up/down, startup health, file watching |
| Environment Variables | `openspec/specs/environment-variables/spec.md` | Coder/bundle/session environment variable precedence and container-injected overrides |
| Raww | `openspec/specs/raww/spec.md` | The relay-side semantic contract for the `raww` verb: operation, target resolution and bundle boundary, authorization mapping, transport behavior, response, input bounds |

## Future capability (not yet a live sibling spec)

`runtime-api` is reserved for the embeddable runtime API capability owned by the `embeddable-runtime-api` change. When that change archives, `openspec/specs/runtime-api/spec.md` will be created from its ADDED requirements, and the row belongs in the `## Partitions` table above at that point. Until then, runtime-api is not a live sibling.

The change's own delta spec is the authority on what it will contain; this note deliberately does not restate its requirements or count them, because a copy here would drift with every edit the change makes and nothing would notice.

## Delta path migration

The rule below is permanent and applies to every change, not to a particular
set of them. A delta targeting a requirement that lives in a partition spec is
authored at `<change>/specs/<partition>/spec.md`, never at
`<change>/specs/session-relay/spec.md`.

`opsx-sync` resolves MODIFIED deltas and adds ADDED requirements **by
requirement name**, so a delta's `### Requirement:` text is portable across
paths; only the containing `specs/<capability>/spec.md` directory changes.

This section previously tracked the one-time migration of the changes that were
in flight when the partition landed, naming each and its pending delta targets.
That migration is complete — the last of them, `deliver-async-terminal-outcomes`
(relay/53), archived on 2026-07-16 — so the roster is gone rather than
maintained. A hub that lists which changes are currently in flight is a document
that is wrong the moment a change archives, and `openspec list` answers that
question without drifting.

## Requirements

### Requirement: Session-Relay Specification Partition Index

The session-relay specification SHALL be the hub reference for the 9 partition specs listed in `## Partitions` above. All normative content for session-relay capability domains (bundle membership, reconciliation, routing, delivery, transport, authorization, look, stream events, choice, raw writes, environment variables) SHALL be authored in the partition spec that matches the capability, not in this hub file.

Active OpenSpec changes SHALL migrate their delta spec files to the partition directory matching each requirement target before archive. Both `## MODIFIED Requirements` and `## ADDED Requirements` deltas follow this rule; the rule covers relocations of existing requirements and (via the future-capability note above) brand-new capabilities whose partition will be created on the active change's archive.

#### Scenario: Partition reference resolves to a sibling spec

- **WHEN** a reviewer looks up a session-relay requirement by name
- **THEN** the requirement text is found in the partition spec identified by the `## Partitions` table, not in this hub file

#### Scenario: Active change delta spec path migration

- **WHEN** an active OpenSpec change has a `MODIFIED Requirements` or `## ADDED Requirements` delta targeting a requirement now in a partition spec
- **THEN** that change's delta spec path SHALL be moved from `<change>/specs/session-relay/spec.md` to `<change>/specs/<partition>/spec.md` before archive
