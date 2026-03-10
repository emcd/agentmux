## 1. Implementation

- [x] 1.1 Add relay host selector support for `--group <GROUP>`.
- [x] 1.2 Enforce mutual exclusivity between positional `<bundle-id>` and
      `--group` with structured validation.
- [x] 1.3 Add optional bundle-local `groups` field parsing from
      `bundles/<bundle-id>.toml`.
- [x] 1.4 Implement group-name validation rules (reserved uppercase vs custom
      lowercase) and reserved implicit `ALL` behavior.
- [x] 1.5 Implement group resolver semantics:
      `--group ALL` selects all bundles; custom groups select by bundle
      membership.
- [x] 1.6 Implement partial-host startup for group mode with lock-held skip
      behavior and per-bundle outcomes.
- [x] 1.7 Implement non-zero exit only when zero bundles are successfully
      hosted in group mode.
- [x] 1.8 Implement canonical machine startup summary payload and text
      rendering (`host_mode`, optional `group_name`, per-bundle
      `outcome`/`reason_code`/`reason`, aggregate counts, `hosted_any`).
- [x] 1.9 Keep existing single-bundle `host relay <bundle-id>` behavior
      unchanged.

## 2. Testing

- [x] 2.1 Add argument-parsing tests for `<bundle-id>` and `--group`
      mutual exclusivity.
- [x] 2.2 Add bundle config tests for optional `groups` and group-name
      validation rules.
- [x] 2.3 Add group resolver tests for `ALL` and custom group selection.
- [x] 2.4 Add integration tests for lock-held partial-host outcomes in
      group mode.
- [x] 2.5 Add tests for startup summary payload shape and non-zero exit only
      when `hosted_bundle_count == 0`.

## 3. Validation

- [x] 3.1 Run `openspec validate add-relay-host-all --strict`.
- [x] 3.2 Run targeted Rust tests for relay host startup flow.
