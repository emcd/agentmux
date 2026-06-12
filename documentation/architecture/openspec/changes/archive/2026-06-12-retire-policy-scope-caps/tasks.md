# Tasks

## 1. Validator sweep

- [x] 1.1 Remove the `allowed` scope-set parameter from
      `parse_scope_for_control` and `parse_action_scope_map`; accept the full
      `none`/`self`/`home`/`all` ladder for every control
- [x] 1.2 Update all `parse_policy_controls` call sites to drop the
      per-control allowed arrays (grant, updown, list, send, find, look,
      raww, do/new/change action maps)

## 2. Documentation sweep

- [x] 2.1 Reword `authorize_route` doc comment and the `checks.rs` module
      header so cross-bundle reach is described as governed by the configured
      scope, not by parse-time caps

## 3. Spec deltas

- [x] 3.1 MODIFIED "Permission Decision Capability Contract": full scope
      ladder, unknown-value rejection retained, in-ladder rejection scenario
      removed
- [x] 3.2 MODIFIED "Authorization Control Vocabulary": full ladder for
      `find`, `list`, `look`, `send`
- [x] 3.3 Verify `cli-surface` and `tui-surface` specs carry no scope-cap
      mandates for `updown` (none found; no deltas needed)

## 4. Tests

- [x] 4.1 Convert the grant cap-rejection test to an unknown-token rejection
      test
- [x] 4.2 Add a full-ladder acceptance test pinning each control to a value
      the retired caps used to reject
