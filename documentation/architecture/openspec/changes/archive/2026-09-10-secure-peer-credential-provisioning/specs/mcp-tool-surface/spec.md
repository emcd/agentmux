## ADDED Requirements

### Requirement: MCP Link Tool

The system SHALL expose a meta-tool `link` that installs a peer credential
into the connected relay's relay-owned slot. `link` SHALL require
`command="peer"`.

`link peer` request `args` SHALL be:

- `alias` (required, the destination relay's local peer alias)
- `psk` (required, the raw PSK issued by the opposite relay)

The relay SHALL require `<alias>@RELAY` registered as a relay principal and
SHALL write exactly `peers/<alias>.psk` with mode 0600, creating missing
relay-owned directories with mode 0700. Installation SHALL classify at
commit time, serialized with the write on a per-slot lock: slot absent
SHALL require `new.peer=all`; slot present with byte-identical content
SHALL require `new.peer=all` OR `change.psk=all`; slot present with
different content SHALL require `change.psk=all`. `home` SHALL be
insufficient. Installation with identical alias and PSK content SHALL be an
idempotent rewrite. An unsafe slot component SHALL abort with
`validation_invalid_credential_path` naming the component.

The PSK SHALL travel to the install operation only as its explicit secret
argument in request memory and structured payloads. It SHALL NEVER be
written to logs, snippets, or diagnostics, and every `link` response SHALL
omit it, whether success or error. Split operation exposes the PSK to the
MCP client by construction; operators choosing split operation explicitly
accept that exposure.

#### Scenario: Install a peer credential into the relay-owned slot

- **WHEN** a caller invokes `link` with `command="peer"`, a registered relay
  alias, and the issued PSK
- **THEN** the relay writes `peers/<alias>.psk` with mode 0600
- **AND** omits the PSK from the response

#### Scenario: Install requires the alias referent

- **WHEN** a caller installs for an alias with no registered
  `<alias>@RELAY` relay principal
- **THEN** the relay rejects the request with a validation error naming the
  alias
- **AND** writes no file

#### Scenario: Install authorization splits on slot state

- **WHEN** installation targets an absent slot without `new.peer=all`, a
  present-identical slot with neither control, or a present-different slot
  without `change.psk=all`
- **THEN** the relay rejects the installation
- **AND** the slot is left unmodified

#### Scenario: Same-authority retry after lost response succeeds

- **WHEN** a caller holding only `new.peer=all` completes an install whose
  response is lost, then retries the identical install
- **THEN** the relay classifies the slot as present-identical and succeeds
- **AND** the slot content is unchanged

#### Scenario: Install responses never carry the PSK

- **WHEN** a `link peer` request succeeds or fails
- **THEN** the response carries no PSK material in any field, diagnostic, or
  log-adjacent payload
