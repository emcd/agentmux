## 1. Confirm the removal is still safe to make

- [ ] No authorization check reads `controls.find` or `controls.do_controls`
      beyond the two discards
- [ ] No policy file in the repository defines a `[policies.controls.do]` block
- [ ] No test asserts a `find` or `do` scope value

## 2. Remove the `do` half

- [ ] Drop `do_controls` from `RawPolicyControls` and its parse
- [ ] Drop `do_controls` from `PolicyControls` and from
      `conservative_default`
- [ ] Drop the `let _ = controls.do_controls.len();` discard

## 3. Remove the `find` half with its template key

- [ ] Drop `find` from `RawPolicyControls` and its parse
- [ ] Drop `find` from `PolicyControls` and from `conservative_default`
- [ ] Drop the `let _ = controls.find;` discard
- [ ] Remove `find = 'self'` from both policy blocks in
      `data/configuration/policies.toml`
- [ ] Verify a relay starting against the updated template parses it, and that
      one carrying the old key fails with a message naming `find`

## 4. Sweep for the same shape elsewhere

- [ ] Check each remaining required key in `RawPolicyControls` for a consuming
      authorization check
- [ ] Report any other required key with no consumer rather than removing it
      here
