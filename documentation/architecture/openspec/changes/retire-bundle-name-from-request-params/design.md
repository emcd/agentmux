## Context

The "Canonical Session Identity" requirement mandates that all session
references in the relay protocol use `session@bundle` form. The request side
of three operations predates this mandate and still accepts `bundle_name` as an
optional qualifier. `retire-bundle-name-from-events` removed the field from
response and event types; this change completes the retirement on the request
side.

## Goals / Non-Goals

- Goals: remove `bundle_name` from all three request variants; enforce
  canonical `target_session` in `IdentityIntrospect`; eliminate the
  now-redundant cross-bundle validation paths.
- Non-Goals: changing the same-bundle-only policy for permissions (policy
  remains, enforcement mechanism changes); renaming `bundle_name` identifiers
  in handler-internal variables (deferred to `todos/editor/1` sweep).

## Decisions

- **PermissionResolve and PermissionList.** Remove the field; no replacement.
  The session connection determines the bundle scope unconditionally. The
  existing `validation_cross_bundle_unsupported` rejection in
  `validate_permission_decision_request` and `handle_permission_list` becomes
  dead code and is removed. Same-bundle-only remains the policy — it is now
  enforced structurally rather than by validating a caller-supplied field.

- **IdentityIntrospect.** Remove `bundle_name`. Require `target_session` to
  carry a `@<namespace>` suffix. Guard with
  `split_principal_id(target_session).is_none()` — the same predicate the
  routing layer uses to detect unqualified targets; it also rejects malformed
  inputs where either side of `@` is empty. A bare `target_session` SHALL be
  rejected with `validation_invalid_params` citing `field: "target_session"`.
  This is consistent with the routing layer's handling of unqualified targets
  (`validation_unqualified_target`) but uses `validation_invalid_params` here
  because the check is a field-format constraint rather than a routing
  decision. The handler then uses `target_session` directly as the
  `target_principal_id` without constructing a canonical id.

- **Cross-lane impact.** MCP `grant` handler already passes `bundle_name: None`
  for all three variants (`grant.rs:94,171`). Removal is compile-forced but
  functionally a no-op for MCP. The only TUI site is
  `src/tui/state/compose/permissions.rs:139`, which passes `bundle_name: None`
  in `PermissionResolve` — compile-forced removal only. Coordinate as
  BE → AE → FE sequence: BE removes relay contract fields; compile errors
  surface cross-lane changes; AE and FE clean up their callers.

## Risks / Trade-offs

- Any caller relying on the bare-id qualification shortcut in
  `IdentityIntrospect` must be updated to pass canonical ids. Alpha: no
  deployed callers outside the repo.
- The `validation_cross_bundle_unsupported` code path for permission operations
  is removed. If a future multi-bundle permission model is introduced, the
  same-bundle enforcement will need to be redesigned from scratch. Alpha:
  acceptable; the field's removal is strictly simpler.
