## ADDED Requirements

### Requirement: Generic handshake exchanges neutral capabilities, readiness, and failures

The handshake SHALL advertise neutral capabilities, readiness states, and typed failures that apply to every adapter profile — Tmux and Pty included. Profile-specific session semantics SHALL NOT appear in the generic handshake. Tmux and Pty adapters SHALL NOT inherit fictitious ACP sessions or capabilities from handshake text.

#### Scenario: Handshake stays neutral across profiles

- **WHEN** a non-ACP adapter completes the handshake
- **THEN** no ACP session semantics (initialize/load/prompt-gate) appear in its established state; neutral target/session correlation identifiers are unaffected

### Requirement: ACP adapter profile scopes ACP lifecycle semantics

The ordered ACP lifecycle — launch, generation publication before the first request, initialize with capabilities negotiation, session select (configured id, else persisted id, else new), session-id persistence, prompt-capability gate — SHALL apply to the ACP adapter profile only, running inside the adapter. The adapter SHALL report neutral ready/failure signals with typed codes. Bootstrap timeouts and retry cadences are ACP-profile defaults informing the timeout schema, not universal wire constants.

#### Scenario: Generation published before first request

- **WHEN** an ACP adapter launches its agent child
- **THEN** it publishes the generation fencing surface before attempting any protocol request

#### Scenario: Permanent failures carry codes, not prose

- **WHEN** bootstrap fails in a way respawn cannot resolve (capability gap, missing binary, denied exec)
- **THEN** the failure carries a permanent-typed code the supervisor keys its no-retry decision on

### Requirement: Capabilities and versions are negotiated before interop

Version, framing, and limits negotiation SHALL be settled before an independently released adapter interoperates with the relay. Skew policy SHALL gate the reference adapter, not follow it.

#### Scenario: Skewed adapter refused with typed failure

- **WHEN** an adapter presents a protocol version outside the negotiated range
- **THEN** the supervisor refuses the binding with a typed version-skew failure rather than operating degraded

### Requirement: Identity and resume custody per the supervision store decision

The relay SHALL hold canonical target identity, current generation currency, and the resume store bytes (never parsing blobs). The adapter SHALL submit resume writes via `persist_state` and receive hydration inside the launch handshake. Cross-restart import does not exist; within-instance replacement imports the current snapshot at publish.

#### Scenario: Stale generation cannot overwrite fresh resume state

- **WHEN** a superseded generation attempts to persist resume state
- **THEN** the write is fenced and the fresh generation's state stands
