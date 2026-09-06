# raww Specification

## Purpose

The relay-side semantic contract for the `raww` verb.

## Requirements

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

Relay-wide (`@GLOBAL`) targets SHALL NOT be rejected at the routing stage.
Rejection occurs after it, against the target's unified registry entry: an
unregistered principal SHALL be rejected with `validation_unknown_target`, and a
registered one whose transport carries `can_be_written = false` with
`validation_unsupported_operation`. This separates namespace routing from
operation-capability concerns.

Validation precedence SHALL evaluate target qualification (at the resolution
stage), then target existence, then capability, then authorization policy checks.

Raww and Look are complementary single-target operations and SHALL share one
config-free resolution stage; their reserved namespace target rejection is
uniform. That stage SHALL resolve `@GLOBAL` targets as relay-wide for both
operations rather than rejecting them, and each operation's handler SHALL then
derive the resolved target's session type and apply its own capability check.

#### Scenario: Reject unqualified raww target

- **WHEN** caller invokes `raww` with a target without `@<namespace>` suffix
- **THEN** relay returns `validation_unqualified_target`

#### Scenario: Reject reserved namespace raww target

- **WHEN** caller invokes `raww` with an `@EXTERNAL` or `@RELAY` target
- **THEN** relay returns `validation_unsupported_namespace`

#### Scenario: Reject registered relay-wide raww target via capability check

- **WHEN** caller invokes `raww` with a registered `@GLOBAL` (relay-wide) target
  whose transport carries `can_be_written = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** the rejection is uniform with the look capability check for the
  same target

#### Scenario: Reject unregistered relay-wide raww target

- **WHEN** caller invokes `raww` with an `@GLOBAL` (relay-wide) target that is
  not a registered principal
- **THEN** relay returns `validation_unknown_target`

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

A raw-kind mailbox entry SHALL be discovered by a transport's delivery-loop
executor through `peek`, exactly as specified by `delivery-quiescence`'s
`Mailbox Peek Operation` requirement: `peek` returns a raw entry only as a
singleton, never combined with mail. Before writing it, the executor SHALL
`declare` it — as its own singleton packing unit, per `Mailbox Submission
Declaration` — exactly as it would declare a mail packing unit; a raw entry
carries no exemption from the declare-before-write discipline.

Once peeked and declared, a transport's write of raw content SHALL map as
follows:

- tmux target: inject literal `text` into target pane; if `no_enter=false`,
  inject Enter after text
- acp target: submit `text` using the existing shared ACP worker/client path
  via `session/prompt`
- pty target: write `text` to the PTY master; if `no_enter=false`, write the
  terminating newline after it
- ui target: unsupported, and refused before a mailbox entry exists. The `raww`
  capability gate rejects a target whose transport is not raw-writable at the
  request boundary, so no raw entry is ever admitted for a `Ui` target and none
  can reach its executor. Should one nonetheless arrive, the executor SHALL
  declare it, write nothing, and acknowledge it `NotSubmitted` — the strongest
  claim it can make, since that arm emits no frame at all — rather than leave it
  at the mailbox head where it would park every entry behind it for the life of
  the target

The transport SHALL treat raww `text` as opaque input and SHALL NOT evaluate
shell expansion or command substitution.

**Ordering.** Mail and raw are variants of one per-target mailbox. `peek`'s
own contract — a raw entry at the head is always returned alone, and mail
past an unpeeked raw entry is never returned — is what enforces the FIFO
barrier structurally: a transport's delivery-loop executor cannot see a raw
entry's successors until that raw entry itself has been acked, and cannot
see mail that precedes an unacked raw entry skipped over.

**Target-side ordering safety within one generation follows from the single
serial delivery executor, not from an additional wait.** Because one
transport instance runs exactly one serial delivery-loop executor for its
lifetime (`delivery-quiescence`'s `Consumer Generation Ownership and
Replacement`), that executor's own write calls are already sequential: it
cannot begin writing a raw entry while a preceding mail write it issued is
still in flight, because it is the same executor issuing both, one after
the other. No separate wait beyond ordinary FIFO peek/ack sequencing is
needed for that case.

**Across a generation replacement, ordering safety is established before
the replacement is ever admitted, not by the raw write waiting on its own.**
`Consumer Generation Ownership and Replacement` already requires a positive
`GenerationFence` verdict for the outgoing generation before a replacement
is admitted at all. By the time a replacement generation's delivery-loop
executor calls its first `peek`, any effect the outgoing generation's
in-flight write might still have produced has already been positively
observed to have ceased. A raw entry therefore needs no fence wait of its
own beyond the one `peek`/`ack` and generation replacement already provide.

#### Scenario: Route raww to acp via session/prompt path

- **WHEN** a peeked raw entry's target transport is `acp`
- **THEN** the transport dispatches via the existing shared ACP
  worker/client `session/prompt` path
- **AND** does not require a new ACP capability surface

#### Scenario: Default raww appends enter

- **WHEN** caller omits `no_enter`
- **THEN** relay treats `no_enter` as `false` when admitting the raw entry
- **AND** the transport appends Enter after injected text

#### Scenario: Raw is not peekable ahead of older mail

- **WHEN** a raww is submitted for a target that has older unacked mail
- **THEN** `peek` continues returning that older mail until it is acked
- **AND** the raw entry is not returned by any `peek` call until it is at
  the mailbox head

#### Scenario: A generation replacement does not need its own raw fence wait

- **WHEN** a transport generation is replaced for a target whose mailbox
  head, after replacement, is a raw entry
- **THEN** the replacement generation's `peek` returns that raw entry as
  soon as it is at the head
- **BECAUSE** the positive fence verdict required to admit the replacement
  already establishes that the outgoing generation's writes have ceased

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
