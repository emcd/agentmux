## ADDED Requirements

### Requirement: Adapter submit replies preserve the submission-evidence trichotomy

The system SHALL resolve submission evidence three ways and only three ways: observed `transport_ack` proves submission (stage disposition); observed provable transport failure proves non-submission (a distinct no-effect terminal); observing neither proves nothing and SHALL resolve submission-unknown — never implicit success. An adapter single-flight refusal (`RefusedBusy`) SHALL count as positive no-effect evidence within the no-effect family — a first-class outcome namespaced as adapter-single-flight so it never reads as relay refusal — not a fourth certainty category.

#### Scenario: Failed write without proof resolves unknown

- **WHEN** the adapter's write to the agent fails without proof that zero bytes left
- **THEN** the reply is `SubmissionUnknown`, not `TransportUnavailable` and not success

#### Scenario: Busy adapter refusal is no-effect evidence

- **WHEN** a submit arrives while one turn is already in flight
- **THEN** the reply is `RefusedBusy` naming the adapter single-flight, nothing is queued, and the outcome is recorded as no-effect — never as success and never as unknown

### Requirement: The supervisor generates SubmissionUnknown on bounded triggers

When the adapter writes and then dies before its ack reaches the relay, the supervisor SHALL generate `SubmissionUnknown` on a bounded trigger — transport disconnect, watchdog expiry, or fence — not on absence-of-ack alone while a write is still pending. `SubmissionUnknown` SHALL require a bounded trigger to resolve; it is NEVER a timeout-less classification, and the absence of an ack while a write is pending resolves nothing. A `TransportUnavailable` no-effect outcome is returned ONLY for provable non-submission (serialization failure, active-prompt refusal with demonstrably unbuffered bytes, or no runtime). Relay→adapter receipt SHALL NOT count as evidence of adapter→agent write.

#### Scenario: Disconnect after write resolves unknown

- **WHEN** the adapter's transport drops after the framed write with no ack observed
- **THEN** the supervisor resolves `SubmissionUnknown` on the disconnect trigger rather than waiting indefinitely

#### Scenario: Delayed ack still resolves once

- **WHEN** a submit ack arrives after the supervisor already resolved the correlation on a bounded trigger
- **THEN** the late ack is recorded as diagnostic evidence and does not create a second delivery disposition

#### Scenario: Duplicate and stale acks collapse

- **WHEN** duplicate acks or acks bound to a superseded generation arrive
- **THEN** they collapse to the single terminal resolution per correlation and never affect the replacement generation

### Requirement: Completion never revisits submission; accept_ack strength is opt-in

A turn completion arriving before its submit ack SHALL be treated as target-health input only and SHALL NOT create a second delivery disposition. An `accept_ack` admitted before its `transport_ack` SHALL be honored as stronger evidence ONLY for content types whose contract opts into accept-requiring semantics; a valid `accept_ack` SHALL be authorized and correlated to the exact envelope/runtime binding. Packing membership SHALL map declaratively: one framed write carries exactly the declared members, so one ack outcome answers for all of them and none outside them.

#### Scenario: Completion before ack stays health input

- **WHEN** a turn completes while its submit ack is still in flight
- **THEN** the completion drives readiness and respawn signals only, and the pending submit resolves on its own ack evidence

#### Scenario: No blind replay after unknown

- **WHEN** a submit resolves `SubmissionUnknown`
- **THEN** the envelope is not resubmitted automatically
- **AND** any retry carries a new correlation
