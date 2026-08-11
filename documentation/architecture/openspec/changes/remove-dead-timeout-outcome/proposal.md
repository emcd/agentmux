## Why

`SendOutcome::Timeout` no longer has a producer. Every bound that once resolved
it — the ACP prime timer, the Tmux readiness and quiescence waits, the Pty prime
wait — has been deleted, and the execution watchdog that replaced them
deliberately terminalizes nothing at the bound: it resolves still-unresolved
members through the fence verdict's evidence order, which spells
`not_submitted` or `submission_unknown` and never `Timeout`. The variant now
survives only as a vocabulary entry that consumers must still map, and it is
reachable from no delivery path.

Leaving it costs more than a dead arm. `Timeout` is a sender- and TUI-visible
outcome spelling, so its presence in the vocabulary is a standing invitation to
resolve a future bound as "the target timed out" — precisely the target-health
inference the delivery commit contract was written to eliminate.

## What Changes

- **BREAKING** Delete the `SendOutcome::Timeout` variant and, with it, the
  `timeout` outcome spelling from the sender-visible delivery vocabulary. A
  terminal delivery outcome is now one of `delivered`, `dropped_on_shutdown`,
  `failed`, `not_submitted`, `submission_unknown`, or `peer_unavailable`.
- **BREAKING** Remove `timeout` from the TUI delivery-state vocabulary. TUI
  terminal states become `success`, `failed`, `not_submitted`, and
  `submission_unknown`, with `accepted` remaining the process-local
  pre-terminal state.
- Delete the relay-side mappings that rendered the variant: the receipt-body
  wording, the sender delivery-outcome event mapping, and the non-delivered
  outcome match arm.
- Delete the TUI's chat-result mapping arm and both stream-side `"timeout"`
  string arms, which become unreachable once their sole producer is gone.
- Correct the TUI's stream-side terminal vocabulary to admit `not_submitted`
  and `submission_unknown`. **This is a deliberate scope extension beyond the
  dispatched deletion** — see below.

### Scope extension, stated rather than absorbed

The dispatched work is the deletion alone. Removing `timeout` from the TUI's
stream-side outcome match, however, exposes that the same match already drops
`not_submitted` and `submission_unknown` on the floor: the relay emits both as
terminal outcomes, and the TUI maps everything it does not recognize to an
unknown-outcome placeholder. So those two outcomes render as `<unknown>` today.

The second consequence is a race rather than a leak, and the distinction
matters for what has to be tested. Clearing a pending delivery does **not**
depend on the outcome spelling — any non-`routed` phase removes the id — so an
already-pending send does resolve. What the outcome spelling gates is entry
into the terminal-message set, and that set is what guards against a terminal
event arriving **before** the queued acknowledgement. Because
`not_submitted` and `submission_unknown` never enter it, a terminal event that
wins that race leaves the later acknowledgement free to insert the message as
pending, where nothing will ever clear it — the terminal event it needed has
already gone by.

That defect predates this change and would survive it untouched. It is included
here for two reasons. First, the requirement being modified is precisely the one
that enumerates the TUI delivery vocabulary, and restating that list while
knowingly omitting two spellings the relay already emits would write a spec that
is false on the day it lands. Second, the implementation touches the same match
arms, so the incremental cost is small and the incremental review surface is one
function.

Reviewed and approved as part of this change rather than split out: rewriting
the enumeration while leaving a known gap in it would make the delta false on
arrival, and the extension is one scenario, two mapping rules, and one task
rather than an unrelated feature.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `tui-surface`: the **TUI Delivery State Mapping** requirement enumerates
  `timeout` as a terminal delivery state and specifies a mapping rule for it.
  Both the vocabulary list and the terminal-transition scenario change. The
  same requirement gains `not_submitted` and `submission_unknown` per the scope
  extension above.
- `look-and-stream-events`: the **Relay Stream Event Contract** requirement
  normatively permits `outcome=timeout` and defines the failure terminal as
  `outcome` in (`timeout`|`failed`). Deleting the sole producer of that spelling
  would otherwise leave a live wire contract for an outcome nothing can emit.
  The `phase` and `outcome` enumerations also gain `not_submitted` and
  `submission_unknown`, which the relay already emits as both.
- `transport-contracts`: the **ACP Stop-Reason Outcome Mapping** requirement is
  **removed entirely**, not edited. Its turn-timeout mapping is what the
  `timeout` sweep found, but on inspection none of its four mappings survives:
  an ACP delivery resolves `Delivered` from typed submission evidence at the
  framed `session/prompt` write, and the turn lifecycle that follows is
  observability only, so no completion reason can select a delivery outcome.
  `acp_stop_cancelled` has no implementation anywhere in the source. It is the
  one `transport-contracts` requirement no in-flight delta covers.

Deliberately **not** listed, having been checked rather than assumed:

- The remaining `SendOutcome::Timeout` citations in `delivery-quiescence`,
  `transport-contracts`, and `transport-abstraction` sit inside requirements
  that `establish-delivery-commit-contract` already removes or replaces, and
  none of that change's deltas reintroduce the spelling in either its qualified
  or bare form. Writing deltas for those would collide with an in-flight change
  and restate a removal already in motion.
- `cli-surface` has no dependency on this variant. Its timeout requirements
  govern per-coder timeout **configuration keys** and retired CLI **flags**, a
  separate subject from the delivery outcome vocabulary.

## Impact

Code, all of which becomes dead at the variant's removal:

- `src/transports/vocabulary.rs` — the variant itself, in the `SendOutcome`
  enum shared by every transport.
- `src/relay/delivery/async_worker.rs` — the non-delivered outcome arm, the
  receipt-body wording, the sender delivery-outcome event mapping, two task
  fixtures, and the exhaustive-vocabulary assertion that pins the outcome-label
  classification.
- `src/tui/state/history.rs` — the chat-result mapping arm, plus the two
  stream-side arms that match the `"timeout"` string off relay event payloads.
- `tests/integration/session_relay_stream/ui_delivery.rs` — an assertion that
  currently tolerates either `timeout` or `failed` for a terminal outcome.

Wire and interoperability surface:

- `SendOutcome` is a deserialized field on the relay and transport contract
  payloads, so an inbound payload carrying `"outcome": "timeout"` — the only
  realistic source being a peer relay at an older revision on a cross-relay
  bang-path target — will fail to deserialize rather than map to a variant.
  Accepted under the project's alpha posture; recorded here because it is the
  one consequence that reaches beyond this process.
- The `relay.send.async.completed` inscription serializes the outcome directly,
  so `timeout` also leaves the relay log vocabulary.

Coordination:

- `add-opencode-compose-readiness-contract` is an active change whose
  `transport-contracts` delta contains a scenario resolving a flush group as
  `SendOutcome::Timeout`. That delta is already stale for an unrelated reason —
  it is written against per-coder `prime-timeout-ms` keys that have since been
  deleted — so this change does not create the conflict, but it does deepen it.
  Reconciling that delta belongs to whoever owns that change, not here.
