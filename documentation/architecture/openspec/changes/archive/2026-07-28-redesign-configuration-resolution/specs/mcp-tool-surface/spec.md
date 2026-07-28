## ADDED Requirements

### Requirement: Retained Startup Fault Surfacing in Tool Responses

Every tool SHALL surface a retained startup fault as its own structured error
when it requires a resolved association, a loaded configuration, or relay
access, rather than returning a generic failure.

The reported cause SHALL identify what could not be resolved and the concrete
inputs involved, so the calling agent can report or repair the condition without
access to server logs. Tools that require none of those things SHALL continue to
succeed.

#### Scenario: Report the retained cause rather than a generic error

- **WHEN** the server holds a retained startup fault for an unconfigured bundle
- **AND** an agent invokes a relay-backed tool
- **THEN** the response is a structured error naming the unconfigured bundle and
  the configuration root that was consulted

#### Scenario: Association-independent tools still succeed

- **WHEN** the server holds a retained startup fault
- **AND** an agent invokes a tool that requires no association, configuration, or
  relay access
- **THEN** the tool succeeds

#### Scenario: Request validation precedes the readiness guard

- **WHEN** the server holds a retained startup fault
- **AND** an agent invokes a tool with arguments that fail the tool's own schema
- **THEN** the response reports the argument fault rather than the retained
  startup fault
