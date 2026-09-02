## REMOVED Requirements

### Requirement: Relay raww operation contract

**Reason**: Relocated verbatim to the new `raww` capability, which owns the
relay-side semantic contract for the verb. `transport-contracts` continues to
govern transport behavior generally; it no longer defines what `raww` is.

### Requirement: Relay raww transport behavior

**Reason**: Relocated verbatim to the new `raww` capability. The behavior it
describes is specific to the verb rather than shared across transports.

### Requirement: Relay raww response contract

**Reason**: Relocated verbatim to the new `raww` capability.

### Requirement: Relay raww input bounds

**Reason**: Relocated verbatim to the new `raww` capability.
