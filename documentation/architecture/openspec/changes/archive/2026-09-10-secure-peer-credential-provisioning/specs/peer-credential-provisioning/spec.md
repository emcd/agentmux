## ADDED Requirements

### Requirement: Peer-Link One-Way Primitive

The system SHALL link destination relay B toward issuing relay A in three
ordered steps: register `<alias>@RELAY` on B via `new peer`, issue
`<connect-as>@RELAY` on A via `new peer`, then install the issued PSK into
B's `peers/<alias>.psk` via the install operation. `alias` and `connect-as`
SHALL remain independent values; neither SHALL be derived from the other.
The registration mint SHALL be explicitly discarded by the coordinator
without rendering, logging, or persisting it; the alias record's credential
remains valid-but-unheld under the normal rotation lifecycle. The issuance
Response SHALL carry exactly one PSK and no other secret — it is the sole
secret-bearing link-flow response. The coordinator SHALL NOT proceed to a
later step when an earlier step fails, except that a second-claim rejection
on registration (record already exists as a relay principal) SHALL proceed
to issuance after verifying the record's relay type.

#### Scenario: One-way bootstrap registers, issues, then installs

- **WHEN** a coordinator links B toward A for alias `bravo` with
  `connect-as` `bravo-link`
- **THEN** B registers relay principal `bravo@RELAY`, A registers relay
  principal `bravo-link@RELAY` holding the credential hash
- **AND** B's `peers/bravo.psk` holds the raw PSK with mode 0600

#### Scenario: Failed issuance halts the bootstrap

- **WHEN** issuance of `<connect-as>@RELAY` on A fails
- **THEN** the coordinator performs no install on B
- **AND** reports the issuance error without rendering any secret

#### Scenario: Existing alias record proceeds to issuance

- **WHEN** registration reports second-claim for an existing relay-principal
  `<alias>@RELAY` record on B
- **THEN** the coordinator verifies the relay type and proceeds to issuance
- **AND** installation targets the same `peers/<alias>.psk` slot

### Requirement: Peer-Link Paired Bidirectional Algorithm

The system SHALL provision both directions in a single paired invocation —
the only fresh bidirectional algorithm — when the operator declares
symmetric naming: verify the declared cross-equality (each side's declared
`connect-as` equals the opposite alias) FIRST with nothing minted,
aborting on mismatch; then register `<alias-B>@RELAY` on B retaining K1;
then register `<alias-A>@RELAY` on A retaining K3; then install K1 into A's
`peers/<alias-A>.psk` and K3 into B's `peers/<alias-B>.psk`. The paired
invocation SHALL mint exactly twice, drop nothing, and perform no issuance
step. Two independent one-way invocations SHALL NOT be composed into a
bidirectional link: reciprocal configuration cross-equals, so the reverse
direction's records already exist with discarded mints. The cross-equality
check SHALL verify operator-stated intent only and derive nothing,
preserving alias/connect-as independence.

#### Scenario: Paired invocation provisions both directions with zero drops

- **WHEN** a coordinator pairs B (`bravo` alias) with A (`alpha` alias)
  declaring symmetric naming
- **THEN** B registers `bravo@RELAY`, A registers `alpha@RELAY`
- **AND** A's `peers/alpha.psk` holds K1 while B's `peers/bravo.psk` holds
  K3, each mode 0600, with no secret rendered

#### Scenario: Cross-equality mismatch aborts before any mint

- **WHEN** a declared `connect-as` value does not equal the opposite alias
- **THEN** the coordinator aborts before either registration
- **AND** no record is created and no secret exists to account for

### Requirement: Peer-Link One-Way to Bidirectional Upgrade

The system SHALL upgrade an existing one-way B→A link to bidirectional
operation by rotating B's `<alias-B>@RELAY` record via `change psk` (fresh
known PSK, `change.psk=all` on B) and installing the rotated PSK into A's
`peers/<alias-A>.psk`. Under reciprocal naming the one-way invocation
already created A's issuance record for the reverse identity, so one
rotation plus one install SHALL complete the reverse direction with no
re-registration.

#### Scenario: Upgrade completes the reverse direction

- **WHEN** a coordinator upgrades an existing one-way B→A link for aliases
  `bravo`/`alpha`
- **THEN** B rotates `bravo@RELAY` and A receives the rotated PSK in
  `peers/alpha.psk` with mode 0600
- **AND** no principal is registered and no secret is rendered

### Requirement: Peer-Link Endpoint and State-Root Selection

The coordinator SHALL address the two relays by explicit state roots,
derive each relay's socket as `<state-root>/relay.sock`, and require both
relays live and reachable. Neither relay SHALL require the other's
`[[peers]]` entry during linking; entries are added after linking and
validated at subsequent startup and first delivery. The required
post-condition is that each entry's `connect-as` equal the opposite relay's
alias.

#### Scenario: Linking precedes peer entry configuration

- **WHEN** a coordinator links two running relays with no `[[peers]]`
  entries for the relationship yet
- **THEN** all link steps succeed against the derived sockets
- **AND** subsequently added entries validate at startup

### Requirement: Peer-Link Per-Side Authorization

Issuance on A SHALL require the connection principal's relay-wide
`new.peer=all` control; alias registration on B SHALL require the same
control on B. Installation on B SHALL classify at commit time, serialized
with the write on a per-slot lock: slot absent SHALL require
`new.peer=all`; slot present with byte-identical content SHALL require
`new.peer=all` OR `change.psk=all`; slot present with different content
SHALL require `change.psk=all`. `home` SHALL be insufficient on every step.
Scope-update authority SHALL NOT substitute for any of these controls.

#### Scenario: Issuance without relay-wide control is refused

- **WHEN** a coordinator issues `<connect-as>@RELAY` on A without
  `new.peer=all`
- **THEN** A rejects the issuance
- **AND** no principal is registered and no install follows

#### Scenario: Same-authority retry after lost response succeeds

- **WHEN** a caller holding only `new.peer=all` completes an install whose
  response is lost, then retries the identical install
- **THEN** B classifies the slot as present-identical and succeeds
- **AND** the slot content is unchanged

#### Scenario: Different-content overwrite requires rotation control

- **WHEN** installation carries content differing from the present slot
  without `change.psk=all`
- **THEN** B rejects the installation
- **AND** the slot is left unmodified

### Requirement: Peer-Slot Install Binding and Permissions

Installation SHALL require `<alias>@RELAY` registered on B as a relay
principal and SHALL write exactly B's `peers/<alias>.psk`, creating missing
relay-owned directories with mode 0700 and the file with mode 0600.
Installation with identical alias and PSK content SHALL be an idempotent
rewrite. An unsafe slot component SHALL abort with
`validation_invalid_credential_path` naming the component.

#### Scenario: Install requires the alias referent

- **WHEN** installation names an alias with no registered `<alias>@RELAY`
  relay principal on B
- **THEN** B rejects the installation with a validation error naming the
  alias
- **AND** writes no file

#### Scenario: Identical reinstall is idempotent

- **WHEN** installation repeats identical alias and PSK content
- **THEN** B succeeds and the slot content is unchanged

### Requirement: Peer-Link Per-Step Timeout Recovery

The protocol SHALL provide no cross-relay transaction and SHALL perform no
automatic compensation or automatic rotation on ambiguous state. Each relay
call SHALL carry its own timeout with mode-specific forward recovery.
One-way registration-unknown SHALL retry `new peer`, where success means
the record was absent and second-claim rejection means a record stands;
existing-record tolerance belongs ONLY to one-way mode, where the install
step verifies the relay type before writing. Paired registration-unknown
SHALL retry once and then report link-registration-unknown: success carries
a fresh known mint, while second-claim proves nothing about the pairing
(a pre-existing record second-claims identically) and automatic rotation
could rotate an unrelated credential. Issuance-unknown SHALL report
link-issuance-unknown: a lost issuance PSK is unknowable, blind
re-issuance is never attempted, and automatic rotation never fires. Both
unknown reports SHALL name explicit operator recovery (`link peer
--upgrade` or drop-and-relink). Install-unknown SHALL retry install with
the same PSK, safe by install idempotency.

#### Scenario: Failed install leaves the issued principal in place

- **WHEN** installation on B fails after issuance on A succeeded
- **THEN** A's `<connect-as>@RELAY` record remains registered
- **AND** the coordinator reports the install error with the recovery
  options, rendering no secret

#### Scenario: One-way registration timeout disambiguates on retry

- **WHEN** one-way alias registration times out and the coordinator retries
- **THEN** success proceeds to issuance directly
- **AND** second-claim rejection verifies the relay type and proceeds to
  issuance

#### Scenario: Paired registration ambiguity reports unknown without rotating

- **WHEN** paired alias registration fails ambiguously and the retry
  second-claims
- **THEN** the coordinator reports link-registration-unknown naming
  explicit recovery
- **AND** sends no rotation for the pre-existing record

#### Scenario: Issuance ambiguity reports unknown without rotating

- **WHEN** issuance fails ambiguously
- **THEN** the coordinator reports link-issuance-unknown naming explicit
  recovery
- **AND** sends no rotation and no blind re-issuance

### Requirement: Peer-Slot Install Durability

Install publication SHALL be temporary-file write plus file fsync, atomic
rename, then destination-directory fsync. A directory-sync failure AFTER
rename SHALL return a typed durability-uncertain outcome WITHOUT rolling
back the file. An acknowledged install SHALL be durable; uncertainty SHALL
be reported, never silent, and SHALL recover via idempotent rewrite.

#### Scenario: Post-rename directory-sync failure stays published

- **WHEN** destination-directory sync fails after a successful rename
- **THEN** B returns a typed durability-uncertain outcome
- **AND** the published slot file is left in place for idempotent retry

### Requirement: Peer-Link Secret Transfer Boundary

The PSK SHALL exist only in coordinator process memory, in the issuance
Response wire payload, and in the install operation's explicit secret
argument in request memory and structured payloads. It SHALL NEVER appear
in stdout/stderr rendering, logs, files other than the destination slot,
snippets, diagnostics, configuration, or persistence written by relay or
adapters. Every link-flow response EXCEPT the issuance Response SHALL omit
the PSK. Split MCP operation exposes the PSK to the MCP client by
construction; operators choosing split operation explicitly accept that
exposure, and single-invocation coordination is the recommended path.

#### Scenario: Bidirectional same-host setup prints no PSK

- **WHEN** an operator links A toward B and B toward A on the same host
- **THEN** both directions complete with relay-owned slot files in place
- **AND** neither invocation renders a PSK to output, logs, or diagnostics

#### Scenario: Split-operation secret travels by explicit argument only

- **WHEN** issuance and installation run as separate invocations
- **THEN** the PSK crosses invocations only as the install operation's
  explicit secret argument
- **AND** no relay- or adapter-written artifact of either invocation carries
  it
