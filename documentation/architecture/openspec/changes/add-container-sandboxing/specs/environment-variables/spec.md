## ADDED Requirements

### Requirement: Container Session Environment Injection

For sessions that reference a container profile, the runtime SHALL inject the
resolved coder/bundle/session environment into the containerized harness process.
The resolved environment is the merged session/bundle/coder environment from the
configurable environment variables contract. It SHALL be applied as container
process environment, not only as environment for the host-side engine launcher or
tmux pane that starts the engine.

Sandbox-injected variables SHALL be applied after configured environment values.
When `ssh-agent = true`, the sandbox-injected `SSH_AUTH_SOCK` value SHALL take
precedence over any configured environment entry with the same name. The same
last-write rule SHALL apply to the injected `AGENTMUX_RELAY_SOCKET` value.

#### Scenario: Inject configured environment into container harness

- **WHEN** a session references a valid container profile
- **AND** its resolved configured environment contains `EXAMPLE_FLAG=enabled`
- **THEN** the engine invocation passes `EXAMPLE_FLAG=enabled` to the
  containerized harness process
- **AND** the value is not limited to the host-side engine launcher environment

#### Scenario: Sandbox SSH_AUTH_SOCK wins over configured environment

- **WHEN** a session references a profile with `ssh-agent = true`
- **AND** the resolved configured environment also contains `SSH_AUTH_SOCK`
- **THEN** the containerized harness process receives the sandbox-injected
  `SSH_AUTH_SOCK` value
- **AND** the configured `SSH_AUTH_SOCK` value is not used inside the container
