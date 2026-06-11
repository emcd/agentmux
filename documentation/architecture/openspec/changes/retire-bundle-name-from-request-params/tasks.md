## 1. Update relay contract (BE)

- [x] 1.1 Remove `bundle_name: Option<String>` from `RelayRequest::PermissionResolve`
      (`src/relay/contract.rs`). Also remove the corresponding destructure arm in
      `src/relay/handlers/dispatch.rs` (around line 58–72).
- [x] 1.2 Remove `bundle_name: Option<String>` from `RelayRequest::PermissionList`
      (`src/relay/contract.rs`). Also remove the corresponding destructure arm in
      `src/relay/handlers/dispatch.rs` (around line 77–85).
- [x] 1.3 Remove `bundle_name: Option<String>` from `RelayRequest::IdentityIntrospect`
      (`src/relay/contract.rs`). Also remove `bundle_name,` from the destructure at
      `src/relay/mod.rs` (around line 406–409).

## 2. Update permission handler (BE)

- [x] 2.1 In `handle_permission_decision` (`src/relay/handlers/permissions.rs`),
      remove the `bundle_name: request_bundle_name` destructure from
      `PermissionDecisionRequestContext` and the corresponding field from that
      struct.
- [x] 2.2 In `validate_permission_decision_request`, remove the `request_bundle_name`
      parameter and the cross-bundle check that returns
      `validation_cross_bundle_unsupported`. Remove the corresponding parameter
      from `handle_permission_decision`'s call site.
- [x] 2.3 In `handle_permission_list`, remove the `request_bundle_name` parameter
      and the cross-bundle check that returns `validation_cross_bundle_unsupported`.
      Update the call site in the handler dispatch.

## 3. Update identity handler (BE)

- [x] 3.1 In `handle_identity_introspect` (`src/relay/handlers/identity.rs`),
      remove the `bundle_name: Option<&str>` parameter. Replace the
      `canonical_session_id` construction with direct use of `target_session`.
- [x] 3.2 Add a pre-check: if `target_session` is not a qualified principal id,
      return `validation_invalid_params` with `field: "target_session"`. Use
      `split_principal_id(target_session).is_none()` as the guard — the same
      predicate the routing layer uses (`routing.rs`) for unqualified-target
      detection. Prefer `split_principal_id` over a bare `contains('@')` check:
      it also rejects inputs where either side is empty (e.g. `"@bundle"` or
      `"session@"`), which a string-contains check would silently accept.
      This guard MUST run before the `introspect_rights` authorization gate
      (validation-before-authorization invariant).

## 4. Cross-lane compile fixes (BE, AE, FE)

- [x] 4.1 `src/mcp/server/handlers/grant.rs` — remove `bundle_name: None` from
      `RelayRequest::PermissionList` and `RelayRequest::PermissionResolve`
      construction sites (currently `None`, so compile-forced removal only,
      no behavioral change). (AE if dispatched separately; BE may carry as
      compile-forced.)
- [x] 4.2 `src/tui/state/compose/permissions.rs:139` — remove `bundle_name: None`
      from the `RelayRequest::PermissionResolve` construction site. This is the
      only known TUI site; verify no others exist before closing. (FE if
      dispatched separately; BE may carry as compile-forced.)

## 5. Tests (BE)

- [x] 5.1 Update `tests/unit/relay_stream/identity.rs`: drop the `bundle_name:
      Option<&str>` parameter from the `introspect_request` helper (~:762) and
      update both call sites (~:842, ~:888) to pass a canonical
      `"alpha@{bundle}"` target instead of the bare `"alpha"`. Test ~:842
      fails outright post-change (bare target hits the format guard); test
      ~:888 keeps its `authorization_forbidden` assertion — with a canonical
      target the format guard passes and the `introspect_rights` gate fires as
      before. The bare-target → `validation_invalid_params` case is covered by
      the new test in 5.2.
- [x] 5.2 Add a test asserting `validation_invalid_params` when
      `IdentityIntrospect` is called with a bare (unqualified) `target_session`.
- [x] 5.3 Confirm that no test asserts `validation_cross_bundle_unsupported`
      for permission operations (those paths are now unreachable).
