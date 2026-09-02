## ADDED Requirements

### Requirement: Relay raww operation contract

Relay SHALL expose a raw direct-write operation named `raww` for a single
explicit target session.

Request contract:
- `target_session` (required)
- `text` (required UTF-8 string)
- `no_enter` (optional boolean, default `false`)
- `request_id` (optional)
- optional bundle selector with same-bundle-only enforcement

`raww` SHALL NOT support broadcast.

#### Scenario: Reject raww broadcast shape

- **WHEN** caller attempts to invoke `raww` without one explicit
  `target_session`
- **THEN** relay rejects the request with `validation_invalid_params`

### Requirement: Relay raww target resolution and bundle boundary

Raww targets SHALL be resolved using the shared single-target routing stage.

Validation behavior:

- bare/unqualified target (no `@<namespace>` suffix) →
  `validation_unqualified_target`
- reserved namespace (`@EXTERNAL`/`@RELAY`) target →
  `validation_unsupported_namespace`
- unknown/non-canonical target → `validation_unknown_target`
- resolved target with `can_be_written = false` →
  `validation_unsupported_operation` (see Transport Capability Contract)
- cross-bundle raww with insufficient scope → `authorization_forbidden`

Relay-wide (`@GLOBAL`) targets are no longer rejected at the routing stage;
rejection occurs at the capability check using `validation_unsupported_operation`
when the resolved session carries `can_be_written = false`. This separates namespace
routing from operation-capability concerns.

Validation precedence SHALL evaluate target qualification (at the resolution
stage), then target existence, then capability, then authorization policy checks.

Raww and Look are complementary single-target operations and SHALL share one
config-free resolution stage; their reserved namespace target rejection is
uniform.

After this change, the routing stage for look and raww SHALL resolve `@GLOBAL`
targets as relay-wide rather than rejecting them at the routing stage; the
handler then derives the resolved target's session type and applies the
capability check. The `RelayWideTargets` enum and `resolve_target`'s
relay-wide-targets parameter are removed in this change — dead code once the
single `Rejected` call site is gone.

#### Scenario: Reject unqualified raww target

- **WHEN** caller invokes `raww` with a target without `@<namespace>` suffix
- **THEN** relay returns `validation_unqualified_target`

#### Scenario: Reject reserved namespace raww target

- **WHEN** caller invokes `raww` with an `@EXTERNAL` or `@RELAY` target
- **THEN** relay returns `validation_unsupported_namespace`

#### Scenario: Reject relay-wide raww target via capability check

- **WHEN** caller invokes `raww` with an `@GLOBAL` (relay-wide) target
- **THEN** relay returns `validation_unsupported_operation`
- **AND** the rejection is uniform with the look capability check for the
  same target

### Requirement: Relay raww authorization mapping

Relay SHALL evaluate raww authorization using policy control `raww`.

Policy scope contract:
- allowed values: `none`, `self`, `home`, `all`
- invalid values (unknown values) SHALL fail configuration validation with
  `validation_invalid_policy_scope`

When raww is denied by policy, relay SHALL return `authorization_forbidden`
with canonical minimum details:
- `capability` = `raww.write`
- `requester_session`
- `bundle_name`
- `reason`

#### Scenario: Deny raww under self scope for non-self target

- **WHEN** requester policy sets `raww = "self"`
- **AND** requester invokes raww to another session in the same bundle
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include `capability = "raww.write"`

#### Scenario: Cross-bundle raww permitted under all

- **WHEN** requester policy sets `raww = "all"`
- **AND** requester invokes raww to a session in a different bundle
- **THEN** relay routes to the target and delivers

#### Scenario: Cross-bundle raww denied under home

- **WHEN** requester invokes raww to a target in a different bundle
- **AND** the requester's configured `raww` scope is `home` or narrower
- **THEN** relay returns `authorization_forbidden`

For a relay-wide (`@GLOBAL`) principal, `home` covers only the `GLOBAL`
namespace, which is populated by relay-wide sessions. Sessions whose registry
entry carries `can_be_written = false` are rejected by the raww capability gate,
so `home` confers no effective raww reach to those targets. `all` remains the
meaningful tier for cross-bundle raww from a relay-wide principal.

### Requirement: Relay raww transport behavior

Relay raww transport execution SHALL map as follows:

- tmux target: inject literal `text` into target pane; if `no_enter=false`,
  inject Enter after text
- acp target: submit `text` using existing shared ACP worker/client path via
  `session/prompt`
- pty target: write `text` to the PTY master; if `no_enter=false`, write the
  terminating newline after it
- ui target: unsupported. `UiTransport::raww` resolves every member `Failed`
  with `reason_code = ui_raw_write_unsupported` and writes nothing

Relay SHALL treat raww `text` as opaque input and SHALL NOT evaluate shell
expansion or command substitution.

**Ordering.** Mail and raw are variants of one per-target relay FIFO.

Raw SHALL preserve FIFO: no authorization across a raw barrier, nor younger work
across older. It SHALL wait for **target-side ordering safety** of older mail,
which requires that execution has ceased — not merely that an outcome has become
terminal. A ledger transition to `submission_unknown` does not prove a
still-running submission cannot take effect later, so terminality is the weaker
condition and is not sufficient here.

Target-side ordering safety is established by the generation fence's positive
verdict, which is why the raw barrier is held until that verdict rather than
until the outcome resolves.

#### Scenario: Route raww to acp via session/prompt path

- **WHEN** raww target transport is `acp`
- **THEN** relay dispatches via existing shared ACP worker/client
  `session/prompt` path
- **AND** does not require a new ACP capability surface

#### Scenario: Default raww appends enter

- **WHEN** caller omits `no_enter`
- **THEN** relay treats `no_enter` as `false`
- **AND** appends Enter after injected text

#### Scenario: Raw preserves FIFO against pending mail

- **WHEN** a raww is submitted for a target that has older `Pending` mail
- **THEN** the older mail is authorized first
- **AND** the raw write follows it

#### Scenario: Raw waits for target-side ordering safety, not terminality

- **WHEN** a raww is submitted for a target with an authorized invocation already
  executing
- **AND** that invocation's members have resolved `submission_unknown`
- **THEN** the raw write still waits for the generation fence's positive verdict
- **AND** it does not proceed on the terminal outcome alone

#### Scenario: Terminal outcome does not release the raw barrier

- **WHEN** an older invocation's members resolve `submission_unknown`
- **AND** its submission execution has not been fenced
- **THEN** a waiting raw write does not proceed on the terminal outcome alone
- **BECAUSE** a terminal outcome resolves the member but does not prove the
  submission cannot still take effect

### Requirement: Relay raww response contract

Relay raww immediate success responses SHALL be queued-only: the response
confirms enqueue acceptance and SHALL NOT include terminal delivery outcome.
Terminal outcomes are reported out-of-band via `delivery_outcome` stream events.

Required success fields:
- `status` (value `queued`)
- `target_session`
- `transport`

Optional success fields:
- `request_id`
- `message_id`

Failure responses SHALL use canonical relay error payload shape (`code`,
`message`, optional `details`). Only enqueue-time failures (e.g. ACP worker
unavailable) surface synchronously.

#### Scenario: Return queued payload for raww dispatch

- **WHEN** raww request to any writable target is accepted at dispatch boundary
- **THEN** relay returns success with `status = "queued"`
- **AND** includes required fields `target_session` and `transport`

### Requirement: Relay raww input bounds

Relay raww SHALL accept UTF-8 multiline text and SHALL reject payloads
larger than 32 KiB (UTF-8 bytes) with `validation_invalid_params`.

#### Scenario: Reject oversized raww text payload

- **WHEN** raww `text` exceeds 32 KiB UTF-8 bytes
- **THEN** relay rejects with `validation_invalid_params`
