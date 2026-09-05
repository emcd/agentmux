## ADDED Requirements

### Requirement: Client-Synthesized Bundle Down Reasons

A client surface SHALL derive the `state_reason_code` of a home bundle it
synthesizes as `down` from the presence of the expected relay socket path alone.

This governs the synthesized entry permitted by `cli-surface` List Sessions
Unreachable Relay Fallback and `mcp-tool-surface` MCP List Sessions Unreachable
Relay Fallback. The entry SHALL report `state=down`, and its
`state_reason_code` SHALL be:

- socket path absent -> `not_started`
- socket path present -> `relay_unavailable`

The derivation SHALL consult only the socket path's presence; a client SHALL NOT
probe further to refine the code, so that every client surface reaches the same
verdict from the same filesystem state.

`state_reason` remains optional and free-form; a client MAY name the socket path
in it.

These two codes describe a *client's* inability to reach a relay and SHALL be
disjoint from the relay-reported codes governed by `bundle-lifecycle` Bundle
Down Reason Precedence (`runtime_no_configured_sessions`,
`runtime_startup_failed`). A client SHALL NOT synthesize a relay-reported code,
and the relay SHALL NOT report a client-synthesized code; no precedence exists
between the two sets because no payload can carry both.

#### Scenario: Report not_started when the relay socket is absent

- **WHEN** a client synthesizes the home bundle because the relay is unreachable
- **AND** the expected relay socket path does not exist
- **THEN** the synthesized entry reports `state=down`
- **AND** `state_reason_code=not_started`

#### Scenario: Report relay_unavailable when the socket exists but the relay does not answer

- **WHEN** a client synthesizes the home bundle because the relay is unreachable
- **AND** the expected relay socket path exists
- **THEN** the synthesized entry reports `state=down`
- **AND** `state_reason_code=relay_unavailable`

#### Scenario: Agree across client surfaces for one filesystem state

- **WHEN** the CLI and the MCP server each synthesize the same home bundle
  against the same relay socket path
- **THEN** both report the same `state_reason_code`
