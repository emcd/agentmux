## MODIFIED Requirements

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
