# choice-decisions Specification

## Purpose

Choice/decision envelope, queue lifecycle, operator classes.

## Requirements

### Requirement: Choice Decision Capability Contract

Relay SHALL evaluate ACP choice-request decision authority using policy
capability `choose`.

- allowed values: `none`, `self`, `home`, `all`
- default when omitted: `none`
- unknown values SHALL fail validation with `validation_invalid_policy_scope`

#### Scenario: Reject unknown choose scope value

- **WHEN** policy configuration sets `choose` to a value outside the canonical
  scope ladder
- **THEN** relay rejects configuration with `validation_invalid_policy_scope`

#### Scenario: Default omitted choose to none

- **WHEN** policy omits `choose`
- **THEN** relay treats `choose` as `none`

### Requirement: Non-Spoofable Decision Actor Identity

Relay SHALL derive choice decision actor identity from the authenticated
stream context and SHALL NOT trust caller-supplied identity fields in the
action payload.

#### Scenario: Derive decision actor from authenticated stream context

- **WHEN** relay processes a `choices.pick` request
- **THEN** relay derives `decided_by` from the authenticated stream context
- **AND** does not consult any caller-supplied identity field in the payload

### Requirement: Same-Bundle Choice Decision Scope

Choice request routing and decisioning SHALL be same-bundle only in alpha.
The bundle scope SHALL be derived from the request's routing namespace
(frame-level namespace, defaulting to the connection's bound bundle); no
caller-supplied in-payload bundle selector is accepted in `ChoicesPick`.
A caller reaches a bundle's choice queue only by routing to that bundle,
subject to its policy controls.

#### Scenario: Permission decision scoped to session's associated bundle

- **WHEN** a choose-authorized principal issues `ChoicesPick`
- **THEN** relay resolves the choice request within the principal's
  associated bundle
- **AND** no `bundle_name` field is accepted in the request payload

### Requirement: Bounded Choice Queue and Replay

Relay SHALL queue ACP choice requests when no choose-authorized UI is
connected.

Queue contract:

- bundle-scoped global FIFO ordering by monotonic enqueue `sequence`
- `pending_max` default `256`
- optional `[relay.choices] pending-max` override in `1..4096`
- enqueue beyond bound SHALL fail with `runtime_choices_queue_full`

Connect/reconnect replay contract:

- relay emits `choices.snapshot` first
- relay then replays pending `choices.requested` oldest→newest
- replay is at-least-once; consumers dedupe by `choice_request_id`

Naming note: the TOML key remains `pending-max` (decoded to `pending_max` in
the deserialized struct). Relay code stores the queue bound as
`choices_pending_max` on `AuthorizationContext`. The ACP chooser closure
captures this per-bundle constant at worker construction and passes it directly
to the queue, so it does not ride `DeliveryContext`, `ChoiceToMake`, or the
per-delivery task; only the genuinely per-delivery decider list does.

#### Scenario: Reject enqueue beyond queue bound

- **WHEN** pending queue depth equals effective `pending_max`
- **AND** another choice request is queued
- **THEN** relay fails with `runtime_choices_queue_full`

#### Scenario: Emit snapshot then replay on authorized ui connect

- **WHEN** choose-authorized UI connects
- **THEN** relay emits `choices.snapshot` before replay
- **AND** replays pending requests in FIFO order

### Requirement: Durable Pending Queue Restoration

Relay SHALL persist pending choice queue state across restart.
If persisted state is unreadable or corrupt, relay SHALL fail fast with
`runtime_choices_queue_unavailable` for that bundle and SHALL NOT silently
drop pending items.

#### Scenario: Fail fast on unrecoverable queue state

- **WHEN** relay startup cannot restore pending choice queue state
- **THEN** relay fails with `runtime_choices_queue_unavailable`

### Requirement: Non-Expiring Choice Pending Lifecycle

Alpha choice requests SHALL be non-expiring while relay and worker
state remain healthy.

Pending requests SHALL remain pending until one of:

- explicit authorized `selected` decision
- explicit authorized `cancelled` decision
- hard terminal cancellation condition (for example
  session/worker termination or aborted choice wait)

Relay SHALL NOT apply timer-based auto-expiry for choice requests
in alpha. The ACP prime timeout field
(`[coders.<id>.acp].prime-timeout-ms`) remains independent from
choice decision lifecycle.

#### Scenario: Keep choice request pending without timer expiry

- **WHEN** choice request is queued and no decision is made
- **AND** relay/worker remain healthy
- **THEN** request remains pending and is not auto-expired by
  timer

### Requirement: Choice Lifecycle Event Carrier

Relay stream events SHALL be canonical machine carrier for choice lifecycle.

Required event names:

- `choices.snapshot`
- `choices.requested`
- `choices.resolved`

Required correlation keys on lifecycle events:

- `message_id`
- `choice_request_id`

Required minimum event fields:

- `choices.snapshot`: `message_id`, `choice_request_ids` (array of pending
  request ids; used by consumers to deduplicate the subsequent replay)
- `choices.requested`: `message_id`, `choice_request_id`,
  `target_session`, `requested_kind`, `requested_details`, `enqueued_at`
- `choices.resolved`: `message_id`, `choice_request_id`, `outcome`,
  `reason_code`, `decided_by`, `resolved_at`

Inscriptions MAY be emitted but SHALL be additive only.

#### Scenario: Emit canonical resolved event with correlation keys

- **WHEN** choice request reaches terminal resolution
- **THEN** relay emits `choices.resolved`
- **AND** event includes `message_id` and `choice_request_id`

### Requirement: Choice Resolution and Enforcement Mapping

Relay SHALL enforce choice terminal outcomes with deterministic mapping to
ACP action and sender-visible terminal outcome/reason_code.

Mapping:

- `selected` → send ACP selected outcome with the chosen `option_id`; prompt
  continues under existing ACP stop-reason mapping contract
- `cancelled` → send ACP cancelled outcome; sender-visible terminal outcome
  `failed` with `reason_code=runtime_choices_request_cancelled`

For raww requests with pending ACP choice turns, the queued response is already
immutable; the sender receives the terminal `delivery_outcome` event only after
the choice is resolved.

#### Scenario: Map cancelled choice to failed terminal outcome

- **WHEN** choice decision resolves to cancelled
- **THEN** relay sends ACP cancelled
- **AND** sender-visible terminal outcome is `failed`
- **AND** `reason_code = runtime_choices_request_cancelled`

#### Scenario: Cancelled choice yields failed delivery_outcome

- **WHEN** relay already returned queued response for an ACP raww
- **AND** the pending ACP choice later resolves to cancelled
- **THEN** relay emits a `delivery_outcome` event with `outcome = "failed"`
- **AND** `reason_code = runtime_choices_request_cancelled`

### Requirement: ACP Choice Option Fidelity

Relay SHALL preserve ACP permission-option fidelity for operator decisioning.

Normative reference:
- ACP Tool Calls: https://agentclientprotocol.com/protocol/tool-calls.md

Conformance note:
- Implementers MUST read and conform to ACP `session/request_permission`
  semantics from the Tool Calls spec before modifying relay choice logic.

Decision contract:

- `choices.pick` SHALL include `outcome`
- allowed decision outcomes are `selected` and `cancelled`
- `selected` SHALL include explicit `option_id`
- `cancelled` SHALL NOT include `option_id`
- relay MUST reject invalid outcome/option combinations with
  `validation_invalid_params`
- relay MUST reject decisions with unknown/non-pending option IDs using
  deterministic validation/runtime taxonomy

Lifecycle payload contract:

- `choices.requested` payload SHALL include ACP option metadata needed for UI
  rendering and explicit option selection

#### Scenario: Resolve with explicit option id from UI decision

- **WHEN** UI submits `choices.pick` with `outcome=selected` and explicit
  `option_id`
- **THEN** relay uses the supplied `option_id` for ACP selected outcome
- **AND** does not transform or substitute the selected option id

#### Scenario: Reject decision missing option id

- **WHEN** UI submits `choices.pick` with `outcome=selected` and missing
  `option_id`
- **THEN** relay rejects with `validation_invalid_params`

### Requirement: Choice Decision Arbitration

First authorized decision SHALL win across both `ui` and `operator`
submitters. Subsequent decisions on resolved requests SHALL be rejected with
`runtime_choices_request_already_resolved` and SHALL NOT mutate state.

#### Scenario: Reject late decision after prior approval

- **WHEN** a second authorized submitter (ui or operator) decides an already
  resolved request
- **THEN** relay rejects with `runtime_choices_request_already_resolved`

### Requirement: Choice Decision Denial Schema

When relay denies choice decisioning by policy, relay SHALL return
`authorization_forbidden` with canonical minimum details:

- `capability`
- `requester_session`
- `bundle_name`
- `reason`

Optional additive details MAY include `target_session`, `targets`,
`policy_rule_id`, and ACP-specific metadata.

The denial schema applies uniformly to `client_class=ui` and
`client_class=operator` submitters.

#### Scenario: Return canonical denial details for unauthorized decision submitter

- **WHEN** a `{ui, operator}` principal lacks `choose` capability
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include canonical required fields

### Requirement: Operator Client Class

Relay SHALL recognize `operator` as a stream `client_class` distinct from
`agent` and `ui`.

Operator class is a decision-submitter role in alpha:

- operator-class streams MAY submit `choices.pick` decisions and
  `choices.list` queries,
- operator-class streams SHALL NOT be inbound delivery targets,
- operator-class streams SHALL NOT receive `choices.snapshot`,
  `choices.requested`, or `choices.resolved` push events; push events
  remain UI-only in alpha.

#### Scenario: Operator class admitted as distinct from agent and ui

- **WHEN** relay enumerates supported stream client classes
- **THEN** the supported set is `{agent, ui, operator}`

### Requirement: Operator-Class Policy Authorization

Bundle policy preset SHALL be the sole source of authority for whether a
configured session may register with `client_class=operator`.

Operator-class authorization SHALL be evaluated at hello time only. Decision
authority (`choose` capability) is evaluated independently per request.

If a configured session attempts `hello` with `client_class=operator` but the
bundle policy preset does not authorize operator class for that principal,
relay SHALL reject with `validation_invalid_client_class_for_hello`.

Operator-class authorization and `choose` capability SHALL remain independent
gates; both must be satisfied for a session to resolve choice requests.

#### Scenario: Reject operator hello when policy preset lacks operator-class authorization

- **WHEN** a configured bundle session sends `hello` with
  `client_class=operator`
- **AND** the bundle policy preset for that principal does not authorize
  operator-class registration
- **THEN** relay rejects with `validation_invalid_client_class_for_hello`

#### Scenario: Operator hello accepted without choose capability

- **WHEN** a configured bundle session sends `hello` with
  `client_class=operator`
- **AND** the bundle policy preset authorizes operator-class registration for
  that principal
- **AND** the policy `choose` capability is `none`
- **THEN** relay accepts the hello
- **AND** subsequent `choices.pick` from that stream is rejected with
  `authorization_forbidden`

### Requirement: Choice List Polling Request

Relay SHALL accept `RelayRequest::ChoicesList` from associated principals
with `client_class ∈ {ui, operator}` and `choose` capability satisfying policy.

`ChoicesList` returns the current set of pending choice requests for
the requester's bundle. The bundle scope is derived from the request's routing
namespace; no caller-supplied bundle selector is accepted.

Response payload SHALL include for each pending request the same field set
emitted by `choices.requested` events:

- `message_id`
- `choice_request_id`
- `target_session`
- `requested_kind`
- `requested_details` (including ACP option metadata)
- `enqueued_at`

Response SHALL include a `schema_version` field and a top-level array of
pending records ordered by enqueue `sequence` ascending.

`ChoicesList` SHALL NOT mutate queue state.

Push events (`choices.snapshot`, `choices.requested`, `choices.resolved`)
remain UI-only in alpha. Operator-class visibility is poll-only via `ChoicesList`.

#### Scenario: Operator client lists pending choice requests

- **WHEN** an operator-class principal with `choose` capability submits
  `ChoicesList`
- **THEN** relay returns pending records in FIFO `sequence` order
- **AND** each record contains the `choices.requested` field set

#### Scenario: Reject choice list from agent class

- **WHEN** a principal with `client_class=agent` submits `ChoicesList`
- **THEN** relay rejects with `validation_invalid_client_class_for_action`

#### Scenario: Reject choice list from operator without choose capability

- **WHEN** an operator-class principal without `choose` capability submits
  `ChoicesList`
- **THEN** relay rejects with `authorization_forbidden`
- **AND** denial details include `capability="choose"`
