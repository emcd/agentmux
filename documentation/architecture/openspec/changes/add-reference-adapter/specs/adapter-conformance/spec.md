## ADDED Requirements

### Requirement: In-tree conformance inputs

Conformance inputs SHALL live in-tree under `tests/fixtures/adapter/`: fixture scripts, request/response transcript pairs, and adversarial frames (version skew, oversize, unknown correlation, duplicate/stale acks, cross-target traffic, generation mismatch). Every input SHALL cite the contract requirement it exercises by requirement name plus capability file (e.g. "Adapter submit replies preserve the submission-evidence trichotomy", `adapter-submit-acks`). Every fixture that asserts persist behavior SHALL cite the payload limit alongside: turn-marker fixtures carry no resume content, so a rich-content assertion against this fixture is a fixture limitation — never evidence about wire protocol or persist wire behavior. The off-tree corpus/checker scratch informs coverage but gates nothing.

#### Scenario: Fixture cites its requirement by name

- **WHEN** a fixture is added for submit-ack trichotomy
- **THEN** it names "Adapter submit replies preserve the submission-evidence trichotomy" (`adapter-submit-acks`), not the capability directory alone

#### Scenario: Persist fixture cites the payload limit

- **WHEN** a fixture asserts persist behavior
- **THEN** it cites the payload limit alongside its requirement citation: turn-marker fixtures carry no resume content, so a rich-content assertion is a fixture limitation, never evidence about wire protocol or persist wire behavior

### Requirement: Operation round-trip equivalence cases

The conformance plan SHALL include at least one round-trip case per wire↔operation mapping from `ideas/mcp/4`: submit-content equivalence (submit frame carries exactly the authorized send dispatch), choice round-trip equivalence (wire Chosen/Cancelled equals direct decide invocation), look-poll equivalence (poll reply renders from direct-look snapshot semantics). Cases assert adapted-is-direct; MCP request/response shapes appear in no case.

#### Scenario: Choice round-trip equals direct decide

- **WHEN** two isolated equivalent pending choices are resolved, one via wire round-trip and one via direct decide-operation invocation
- **THEN** both resolutions carry the same outcome, decider, and reason code

### Requirement: Conformance exclusion list

The conformance plan SHALL include no operation-equivalence case for `help` (no operation), supervision-only methods (handshakeHello, persistState custody, lifecycleEvent, supervisionSignal — no operation counterpart), the `relay.ping` liveness probe (a request with no operation counterpart), or adapter-local errors (not in the canonical error set). This exclusion governs operation-equivalence cases only: protocol-level fixtures for ping/pong and supervision signals remain required.

#### Scenario: Excluded method has no equivalence case

- **WHEN** the plan is enumerated
- **THEN** no case names `help`, a supervision-only method, or `relay.ping` as an operation counterpart
- **AND** a ping/pong protocol fixture exists outside the equivalence set

### Requirement: Seam-exercise rule

Any case added at a shape-rule-vs-admission-rule seam SHALL be exercised against the rules already stated for that class before it is filed as an exemplar.

#### Scenario: Seam case exercised before filing

- **WHEN** a new adversarial frame targets an admission boundary
- **THEN** the plan records the exercised rule outcome before the frame becomes an exemplar
