# Change: Retire per-control policy scope caps

## Why

The policies file is authoritative: operators configure each control's scope,
and the authorization checks give each value its effect via scope rank order.
Despite that directive, `parse_policy_controls` imposed hardcoded per-control
allowed-scope caps: `grant` and `updown` rejected `self`/`all`, `list` and
`send` rejected `none`/`self` (making them undisableable by policy), and the
`new`/`change` action maps rejected `self`. The caps were functionally inert —
`authorize_grant`/`authorize_updown` are flat `home`-minimum checks, so `all`
passes by rank order and `self` already behaves as deny — but they rejected
valid operator intent at parse time. On 2026-06-11 a production operator swept
controls to the `all` scope; the parse-time cap on `grant` rejected the file,
unloading three bundles and crash-looping the relay. Tracked as
issues/relay/33 through 36.

## What Changes

- Every policy control accepts the full `none`/`self`/`home`/`all` scope
  ladder at parse time. Unknown tokens still fail validation with the
  control's existing error code.
- The `allowed` scope-set parameter is removed from the policy scope parser
  entirely, so per-control caps cannot be reintroduced without restoring the
  mechanism.
- The `session-relay` spec requirement "Permission Decision Capability
  Contract" drops its alpha-scope allowed-values cap and the rejection
  scenario for in-ladder values; "Authorization Control Vocabulary" lists the
  full ladder for `find`, `list`, `look`, and `send`.
- Authorization doc comments no longer describe parse-time caps as the
  mechanism governing cross-bundle reach.
- No authorization behavior changes: consuming checks already rank-order all
  four scope values.

## Impact

- Affected specs: `session-relay` (modified)
- Affected code: `src/relay/authorization/loading.rs`,
  `src/relay/authorization/checks.rs` (doc comments only),
  `tests/unit/relay/request_validation.rs`
- Policy files previously rejected at parse time now load; authorization
  outcomes for previously accepted files are unchanged.
