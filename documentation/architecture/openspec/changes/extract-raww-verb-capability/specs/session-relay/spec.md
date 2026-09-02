## MODIFIED Requirements

### Requirement: Session-Relay Specification Partition Index

The session-relay specification SHALL be the hub reference for the 9 partition specs listed in `## Partitions` above. All normative content for session-relay capability domains (bundle membership, reconciliation, routing, delivery, transport, authorization, look, stream events, choice, raw writes, environment variables) SHALL be authored in the partition spec that matches the capability, not in this hub file.

Active OpenSpec changes SHALL migrate their delta spec files to the partition directory matching each requirement target before archive. Both `## MODIFIED Requirements` and `## ADDED Requirements` deltas follow this rule; the rule covers relocations of existing requirements and (via the future-capability note above) brand-new capabilities whose partition will be created on the active change's archive.

#### Scenario: Partition reference resolves to a sibling spec

- **WHEN** a reviewer looks up a session-relay requirement by name
- **THEN** the requirement text is found in the partition spec identified by the `## Partitions` table, not in this hub file

#### Scenario: Active change delta spec path migration

- **WHEN** an active OpenSpec change has a `MODIFIED Requirements` or `## ADDED Requirements` delta targeting a requirement now in a partition spec
- **THEN** that change's delta spec path SHALL be moved from `<change>/specs/session-relay/spec.md` to `<change>/specs/<partition>/spec.md` before archive
