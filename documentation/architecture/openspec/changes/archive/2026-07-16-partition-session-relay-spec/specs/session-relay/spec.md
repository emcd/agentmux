## REMOVED Requirements

All 97 base requirements relocate to 8 live partition specs (see `add-cross-relay-list-discovery`-style sibling specs at `openspec/specs/<partition>/spec.md`). The 97 include 94 base requirements from `44d59dd` and 3 ADDED by `add-pty-transport` archive at `774f116` (Pty Prime Timeout, Pty Wedged State Detection, Pty Default Per-Coder Dimensions). `runtime-api` is reserved for the active `embeddable-runtime-api` change; its 10 ADDED requirements never lived in the 97-requirement live session-relay base and are NOT listed in this REMOVED block. Names are preserved byte-for-byte so any existing cross-reference by requirement name remains valid; only the spec path changes.

### Requirement: Bundle Membership Configuration

### Requirement: Bundle Group Membership Field

### Requirement: Session Routing Primitive

### Requirement: Suffix-Based Target Routing

### Requirement: Request Routing Namespace

### Requirement: GLOBAL Namespace List

### Requirement: Canonical Session Identity

### Requirement: Unified Namespace-Keyed Session Registry

### Requirement: Verified Identity Trust Boundary

### Requirement: Session Type Taxonomy

### Requirement: Per-Session Readiness In List Payload

### Requirement: Bundle Hosted Flag In List Payload

### Requirement: Relay raww target resolution and bundle boundary

### Requirement: JSON Send Envelope

### Requirement: Quiescence-Gated Delivery

### Requirement: Quiescence Documentation

### Requirement: Delivery Results Without ACK Protocol

### Requirement: Async Queue Lifecycle and Ordering

### Requirement: Async Delivery Observability

### Requirement: Async Queue Growth Risk Disclosure

### Requirement: Configurable tmux socket

### Requirement: Prompt-Readiness Template Gating

### Requirement: Prompt-Readiness Template Validation

### Requirement: Coder Command Template Resolution

### Requirement: Coder-Scoped Prompt-Readiness Templates

### Requirement: ACP Send Lifecycle Selection Precedence

### Requirement: ACP Session Identity Persistence Ownership

### Requirement: ACP Load Path Fail-Fast Semantics

### Requirement: ACP Capability Gating

### Requirement: ACP Stop-Reason Outcome Mapping

### Requirement: ACP Terminal Readiness Tracking

### Requirement: ACP Persistent Worker Lifecycle

### Requirement: ACP Permission Request Readiness Signal

### Requirement: Relay raww operation contract

### Requirement: Relay raww transport behavior

### Requirement: Relay raww response contract

### Requirement: Relay raww input bounds

### Requirement: ACP Transport Error Code

### Requirement: Transport Capability Contract

### Requirement: ACP Prime Timeout

### Requirement: Tmux Prime Timeout

### Requirement: Tmux Wedged State Detection

### Requirement: Copy-Mode-Transparent Injection

### Requirement: Pty Prime Timeout

### Requirement: Pty Wedged State Detection

### Requirement: Pty Default Per-Coder Dimensions

### Requirement: Policy Preset Source

### Requirement: Session Policy Binding

### Requirement: Authorization Control Vocabulary

### Requirement: Centralized Authorization Decision Point

### Requirement: Authorization Evaluation Order

### Requirement: Authorization Denial Schema

### Requirement: Relay List Authorization

### Requirement: Relay Send Scope Control

### Requirement: Authorization Hooks for Do and Find

### Requirement: Uniform Cross-Bundle Authorization Model

### Requirement: UI Request-Path Sender Validation

### Requirement: Relay List Sessions Request Scope

### Requirement: Relay raww authorization mapping

### Requirement: Relay Look Operation

### Requirement: Look Capture Window Bounds

### Requirement: Look Response Contract

### Requirement: ACP Look Snapshot Contract

### Requirement: ACP Look Freshness Derivation

### Requirement: Persistent Relay Client Streams

### Requirement: Hello Registration Contract

### Requirement: Static Recipient Routability

### Requirement: Relay Stream Event Contract

### Requirement: Stream Failure Semantics

### Requirement: Choice Decision Capability Contract

### Requirement: Non-Spoofable Decision Actor Identity

### Requirement: Same-Bundle Choice Decision Scope

### Requirement: Bounded Choice Queue and Replay

### Requirement: Durable Pending Queue Restoration

### Requirement: Non-Expiring Choice Pending Lifecycle

### Requirement: Choice Lifecycle Event Carrier

### Requirement: Choice Resolution and Enforcement Mapping

### Requirement: ACP Choice Option Fidelity

### Requirement: Choice Decision Arbitration

### Requirement: Choice Decision Denial Schema

### Requirement: Operator Client Class

### Requirement: Operator-Class Policy Authorization

### Requirement: Choice List Polling Request

### Requirement: Bundle Reconciliation

### Requirement: Reconciliation Lifecycle Policy

### Requirement: Relay Bundle Lifecycle Operations

### Requirement: Relay Bundle Lifecycle Result Contract

### Requirement: Bundle Configuration Includes Autostart Eligibility

### Requirement: Bundle Startup Evaluation Boundary

### Requirement: Bundle Startup Health Model

### Requirement: Startup Failure Visibility Contract

### Requirement: Bundle Down Reason Precedence

### Requirement: Dynamic Bundle File Watching

### Requirement: Coder Environment Variables

### Requirement: Bundle Environment Variables

### Requirement: Session Environment Variables

### Requirement: Environment Variable Precedence

## ADDED Requirements

### Requirement: Session-Relay Specification Partition Index

The session-relay specification SHALL be the hub reference for the 8 partition specs listed in the file's `## Partitions` table. All normative content for session-relay capability domains (bundle membership, reconciliation, routing, delivery, transport, authorization, look, stream events, choice, environment variables) SHALL be authored in the partition spec that matches the capability, not in this hub file. The `runtime-api` partition is reserved as a future capability owned by the active `embeddable-runtime-api` change; when that change archives, the live `openspec/specs/runtime-api/spec.md` is created and the 10 ADDED requirements land there.

Active OpenSpec changes SHALL migrate their delta spec files to the partition directory matching each requirement target before archive. Both `## MODIFIED Requirements` and `## ADDED Requirements` deltas follow this rule; the rule covers relocations of existing requirements and (via the future-capability note above) brand-new capabilities whose partition will be created on the active change's archive.

#### Scenario: Partition reference resolves to a sibling spec

- **WHEN** a reviewer looks up a session-relay requirement by name
- **THEN** the requirement text is found in the partition spec identified by the `## Partitions` table, not in this hub file

#### Scenario: Active change delta spec path migration

- **WHEN** an active OpenSpec change has a `MODIFIED Requirements` or `## ADDED Requirements` delta targeting a requirement now in a partition spec
- **THEN** that change's delta spec path SHALL be moved from `<change>/specs/session-relay/spec.md` to `<change>/specs/<partition>/spec.md` before archive