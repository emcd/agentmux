## Why

`find` and `do` are authorization controls for verbs the system does not
provide. Both are parsed and then discarded: `parse_scope_for_control` reads
`find`, `parse_action_scope_map` reads `do`, and `resolution.rs` throws both
away with two bare `let _ =` bindings.

`find` is worse than inert. `RawPolicyControls` carries
`deny_unknown_fields` and `find: String` has no serde default, so it is a
required key: every deployment must supply a value for a control nothing reads,
and the shipped template sets `find = 'self'` in both policy blocks.

The operator's ruling is that a key must not be required unless something
implements it — the defect is the requirement, not its removal.

## What Changes

- **BREAKING** the `find` key is removed from `[policies.controls]`. Because
  `deny_unknown_fields` rejects keys the struct does not declare, the field and
  the shipped template must change in the same release; removing one without
  the other converts a working startup into a refusal.
- `do` action-id scoped controls are removed. This half is not breaking: the
  field carries a serde default and no shipped policy defines a
  `[policies.controls.do]` block.
- `Authorization Hooks for Do and Find` is removed rather than amended. Its two
  scenarios describe relay denying by the `do` control map, which a discarded
  map cannot do, so they were never satisfiable.
- The control vocabulary gains a rule against the recurrence: a control does not
  belong in the vocabulary before a check consumes it.
- Incidental correction, forced by re-stating the block: the built-in
  conservative default is recorded as `look = self`, but the change that set the
  default look scope to all:home widened it in both
  `PolicyControls::conservative_default` and the shipped `policies.toml` without
  updating the specs. Nothing in the corpus, the code, or the tests asserts
  `self`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `authorization-scope`: `Policy Preset Source` (built-in default list),
  `Authorization Control Vocabulary` (control list), and the removal of
  `Authorization Hooks for Do and Find`.

## Impact

Configuration-breaking. Sequencing matters more than usual: the production relay
is held stale until after 0.9.0, so this should ride a release that already
breaks `policies.toml` rather than being the reason for one. The ruling settled
whether, not when.

Code and template changes belong to the relay lane, not to this one:

- `src/relay/authorization/loading.rs` — the `find` field and its parse, the
  `do_controls` field and its parse
- `src/relay/authorization/context.rs` — `PolicyControls::find` and
  `do_controls`, and their entries in `conservative_default`
- `src/relay/authorization/resolution.rs` — the two `let _ =` discards, which
  must go with the fields rather than be left to be deleted later as apparent
  dead code
- `data/configuration/policies.toml` — `find = 'self'` in both policy blocks

Not addressed here: the vocabulary lists five controls while the loader parses
eleven, so `raww`, `choose`, `updown` and the `new`/`change`/`drop` action maps
are absent from a requirement that reads as exhaustive. That is the same defect
class from the opposite direction — backed controls missing rather than unbacked
controls present — and it wants its own change.
