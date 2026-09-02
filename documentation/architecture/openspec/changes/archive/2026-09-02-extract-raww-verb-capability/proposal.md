## Why

The specification corpus is organized by layer and surface, so a verb's contract
is smeared across capabilities rather than owned by one. `raww` is the clearest
case: 19 requirement titles across six capabilities, plus raww-specific content
inside a seventh that carries no raww-titled requirement at all.

That organization is not merely untidy — it is the mechanism behind several
defects already found. A verb specified once per surface gets restated per
surface, and restatements diverge silently, since `openspec validate` checks
that requirements are well formed and never that two of them agree. The
duplicated timeout paragraph between `mcp-tool-surface` and `cli-surface`, the
`Authorization Control Vocabulary` drift where verbs were added at the surface
layer and the central list never noticed, and `raww`'s authorization mapping
living a capability away from its own operation contract are all the same root
cause.

Extracting `raww` found a further instance while this change was being written:
`relay-routing-layer` and the `raww` authorization mapping each carried a
scenario asserting that cross-bundle raww under `all` routes and delivers. Two
copies, two capabilities, neither aware of the other.

There is precedent for the fix. `choice-decisions` is already a verb spec — it
owns `choose` relay-side end to end, and it is the cleanest authorization story
in the corpus. `choose` never drifted into the central control vocabulary
because it was correctly distributed from the start.

## What Changes

A new `raww` capability owns the relay-side semantic contract for the verb:
what it does, where it may be targeted, who may invoke it, how it writes, what
comes back, and its input bounds.

- **New capability `raww`**, six requirements relocated verbatim: the operation
  contract, transport behavior, response contract and input bounds from
  `transport-contracts`; target resolution and bundle boundary from
  `addressing-routing`; the authorization mapping from `authorization-scope`.
- **`relay-routing-layer`** gives up two raww-specific scenarios from
  `Authorization Stage`. One (`Cross-bundle Raww denied under home`) relocates
  to the `raww` authorization mapping, carrying the note explaining why `home`
  confers no effective reach to `can_be_written = false` targets. The other
  (`Cross-bundle Raww permitted under all`) is retired rather than moved,
  because the destination already states it.
- **`authorization-scope`** keeps the verb-independent model and stops
  enumerating verbs in `Cross-bundle operation denied under home scope`, which
  named `look`, `send` and `list` and silently omitted `raww` — which is
  plausibly why raww grew its own copies elsewhere.
- **`session-relay`**, the partition hub, reads nine partitions rather than
  eight — in its index requirement, and in the preamble, live-total grep recipe
  and partitions table that are not delta-governed.
- **The thirteen surface requirements stay put.** An MCP payload field is a
  fact about MCP, not a fact about `raww`. Surfaces continue to reference the
  verb without defining it, which is the split the corpus already asserts
  wherever it says CLI and MCP are validators and the relay is the decision
  point.

One further consolidation, found in review: the relocated denial scenario
duplicated one already present in the raww target resolution requirement — both
asserting that `home` or narrower is denied cross-bundle. The
authorization-mapping scenario is kept, with its note, and the resolution one is
dropped. An authorization outcome belongs in the authorization mapping; target
resolution should resolve targets.

No behavior change. Relocated requirements are byte-identical to their sources
apart from the two deliberate consolidations, verified by diff rather than by
inspection.

## Sequencing

**This change syncs first.** `redesign-mailbox-delivery-protocol` migrates its
MODIFIED `Relay raww transport behavior` delta from `specs/transport-contracts/`
to `specs/raww/` afterwards, and **this change SHALL NOT archive** until that
migration is reviewed and landed. `session-relay`'s partition index requirement
obliges migration before the affected change archives — not before the source
of the move syncs.

Sync-first is forced, not preferred. `verify-openspec-deltas.py` runs as a
pre-commit hook on any changed delta path and rejects a MODIFIED delta whose
live counterpart is absent. Live `raww` does not exist until this change syncs,
so the migration cannot be committed before then; requiring it first is a cycle
whose only other exit is bypassing the hook.

Sync-first is also the safe direction, which is why the cycle resolves rather
than merely moving. The move is content-preserving: the other change's edits
live only in its delta and were never in the text this change relocates, so
syncing first cannot drop them, and its delta then replaces the requirement in
`raww` with its version. The reverse order is the dangerous one — it would leave
this change copying a stale requirement into `raww` and dropping those edits
with nothing to report it, since the retention audit compares a delta against
the capability it modifies, and a move is a REMOVED in one capability and an
ADDED in another, so no check compares the two halves.

One citation in `transport-abstraction` becomes stale and IS repaired here.
`Transport Interface Contract`'s trait list names `transport-contracts` as the
authority for the raww operation contract; this change repoints it to `raww`.

`redesign-mailbox-delivery-protocol` also has a MODIFIED delta for that
requirement, so two active changes modify it — normally the collision worth
avoiding. It is benign here, and only because of what the other delta does: its
version removes `raww` from the `Transport` trait entirely and cites nothing for
the operation contract. So whichever way that sync resolves, the repointed
sentence is superseded by text that does not carry the stale citation at all.
Repairing it here fixes live at the moment this change breaks it, rather than
leaving a false citation standing for however long the other change takes.

That other delta introduces its own stale citation, to `transport-contracts`'
`Relay raww transport behavior`, which is not reachable from here and is
recorded as a prerequisite of this change's archive. The prompt-readiness
reference in the same requirement is correct as it stands and must be left
alone — prompt-readiness templates are not part of this extraction.

## Capabilities

### New Capabilities

- `raww`: the relay-side semantic contract for the `raww` verb — operation,
  target resolution, authorization, transport behavior, response, input bounds.

### Modified Capabilities

- `transport-contracts`: gives up four raww requirements; continues to govern
  transport behavior generally.
- `addressing-routing`: gives up raww target resolution; continues to own the
  general addressing grammar and session type taxonomy.
- `authorization-scope`: gives up the raww authorization mapping; retains the
  scope ladder, evaluation order, denial schema and uniform cross-bundle tiers,
  with one scenario de-enumerated.
- `relay-routing-layer`: `Authorization Stage` gives up two raww-specific
  scenarios and keeps its general rule.

## Impact

Specification only. No source file changes.

`Relay raww authorization mapping` moves out of `authorization-scope` while that
capability is also the subject of ongoing distribution work; this change is the
first instance of that distribution and sets the pattern the rest will follow.

Deliberately deferred: `authorization-scope` also carries `Reject cross-bundle
send under home-only scope` and `Permit cross-bundle send under all-all scope`,
a send-specific pair duplicating the same general rule. They are not raww's
business and are tracked separately, to be resolved when `send` is unsmeared or
sooner.
