## MODIFIED Requirements

### Requirement: Look Command Surface

The system SHALL expose a read-only inspection command:

- `agentmux look <target-session>`

`agentmux look` SHALL support:

- optional `--bundle <name>` (selects the requester's dispatch bundle, not the
  target's)
- optional `--lines <n>`

`<target-session>` MAY be a bare session id (inspected within the requester's
dispatch bundle) or a peer-qualified `<session>@<bundle>` id that inspects a
session in a peer bundle.

`agentmux look` SHALL return canonical structured JSON output in MVP.
`agentmux look` authorization SHALL use capability label `look.inspect`.
Policy control `look` determines allowed scope (`self`, `all:home`, `all:all`).
Cross-bundle look (a `<session>@<bundle>` target naming a peer bundle) requires
`all:all` scope; same-bundle non-self look requires `all:home`. The CLI is a
thin adapter and propagates relay authorization and resolution outcomes
unchanged.

#### Scenario: Inspect target session from CLI

- **WHEN** an operator runs `agentmux look <target-session>`
- **THEN** the system requests a read-only snapshot for that target session
- **AND** returns structured JSON payload from relay inspection response

#### Scenario: Use associated bundle when bundle flag is omitted

- **WHEN** an operator runs `agentmux look <target-session>` without `--bundle`
- **THEN** the system uses associated bundle context resolved for the caller

#### Scenario: Reject invalid lines value

- **WHEN** an operator provides `--lines` outside valid range
- **THEN** the system rejects invocation with `validation_invalid_lines`

#### Scenario: Inspect peer bundle session via qualified target

- **WHEN** an operator runs `agentmux look <session>@<peer-bundle>` naming a
  bundle other than the requester's dispatch bundle
- **AND** the requester is authorized at `look = all:all`
- **THEN** the system returns the peer bundle's snapshot from the relay response

#### Scenario: Surface peer resolution errors from relay

- **WHEN** the qualified target names a bundle not configured on the relay, or a
  session that is not a member of the named peer bundle
- **THEN** the CLI surfaces `validation_unknown_bundle` or
  `validation_unknown_target` respectively, unchanged from the relay

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** operator requests look for same-bundle non-self target
- **AND** requester policy `look` scope is `self`
- **THEN** CLI surfaces `authorization_forbidden`
