## 1. Confirm the removal is still safe to make

- [x] No authorization check reads `controls.find` or `controls.do_controls`
      beyond the two discards
- [x] No policy file in the repository defines a `[policies.controls.do]` block
- [x] No test asserts a `find` or `do` scope value

The third check needed restating to be useful. No test *asserts* either scope,
but twenty-four test files *supply* `find` in the `policies.toml` fixtures they
generate, and under `deny_unknown_fields` every one of them would have failed to
parse. Fifty-six occurrences across twenty-five files, template included.

## 2. Remove the `do` half

- [x] Drop `do_controls` from `RawPolicyControls` and its parse
- [x] Drop `do_controls` from `PolicyControls` and from `conservative_default`
- [x] Drop the `let _ = controls.do_controls.len();` discard

## 3. Remove the `find` half with its template key

- [x] Drop `find` from `RawPolicyControls` and its parse
- [x] Drop `find` from `PolicyControls` and from `conservative_default`
- [x] Drop the `let _ = controls.find;` discard
- [x] Remove `find = 'self'` from both policy blocks in
      `data/configuration/policies.toml`
- [x] Verify a relay starting against the updated template parses it, and that
      one carrying the old key fails with a message naming `find`

Both verified through `agentmux check configuration`, which reaches policy
parsing without starting a relay and is therefore the migration tool to point
operators at. The rejection names the file, the line, the offending key, and the
accepted set:

    validation_invalid_arguments: failed to parse authorization policy artifact
    TOML parse error at line 10, column 1
    10 | find = 'self'
       | ^^^^
    unknown field `find`, expected one of `list`, `look`, `send`, `raww`,
    `choose`, `updown`, `new`, `change`, `drop`

## 4. Sweep for the same shape elsewhere

- [x] Check each remaining required key in `RawPolicyControls` for a consuming
      authorization check
- [x] Report any other required key with no consumer rather than removing it
      here

Nine controls remain and every one is consumed. The three still required
(`list`, `look`, `send`) all reach `scope_for_capability`; `list` additionally
gates relay-wide discovery through `requester_list_reaches_all`. The six
optional ones (`raww`, `choose`, `updown`, and the `new`/`change`/`drop` action
maps) are consumed by the capability and relay-action checks. No other required
key lacks a consumer, so nothing further is removed here.

One finding for a separate change, not actioned: the vocabulary requirement
lists three of those nine and reads as exhaustive, so six consumed controls are
absent from it. That is the same defect from the opposite direction — backed
controls missing rather than unbacked controls present. Tracked as
`agentmux:todos/openspec/17`.
