## 1. Add capability derivation for SessionType (BE)

- [ ] 1.1 Implement capability derivation as methods on `SessionType` — e.g.
      `fn can_be_looked(self) -> bool`, `fn can_be_written(self) -> bool`,
      `fn can_stream_output(self) -> bool` — or as a
      `TransportCapabilities::of(SessionType)` helper struct. Do NOT add bool
      fields to any struct; capabilities are pure functions of `SessionType` and
      should be derived at call sites. Every check site already has a
      `SessionType` in hand: bundle targets via
      `BundleMember.target.session_type()`, relay-wide targets via
      `TuiSession::session_type` (from `users.toml`), live registry entries via
      `RegistryEntry.session_type`. Note: `StreamRegistration`
      (`connection.rs` / `stream.rs`) is a thin unregistration handle, not the
      right home for this derivation.
- [ ] 1.2 Implement the derivation table:

      | Transport | can_be_looked | can_be_written | can_stream_output |
      |-----------|---------------|----------------|-------------------|
      | Tmux      | true          | true           | false             |
      | Acp       | true          | true           | true              |
      | Pty       | true          | true           | true              |
      | Ui        | false         | false          | false             |
      | Pubsub    | false         | false          | false             |

      The `Pty` row is normative and forward-looking; no `Pty` variant exists
      in `TargetConfiguration` or `SessionType` today. Forward-declare it in a
      comment in the derivation; the row activates when the `Pty` transport
      lands (expected in `decouple-transport-layer`).

      `can_stream_output` is derivable now but not yet consumed by any handler
      in this proposal; it is available for the streaming look follow-on.

## 2. Add validation_unsupported_operation error code (BE)

- [ ] 2.1 Add `validation_unsupported_operation` as a named error code
      constant in `src/relay/` (alongside existing validation error constants).

## 3. Update look handler (BE)

- [ ] 3.1 In `src/relay/handlers/look.rs`, add a pre-authorization capability
      check. Derive `can_be_looked` from the target's `SessionType` and return
      `validation_unsupported_operation` with informative details if false.

      Scoping — two target paths:
      - **Bundle targets**: `SessionType` is available from
        `BundleMember.target.session_type()` at the `prepare_look` step.
      - **Relay-wide (`@GLOBAL`) targets**: `prepare_look` resolves via
        `bundle.members` (look.rs:164-176) and does not find relay-wide targets.
        After task 4.3 removes the routing rejection, a `@GLOBAL` target passes
        routing but hits `validation_unknown_target` in `prepare_look` before
        the capability check. Add an early branch on `target_route.relay_wide`
        before the bundle-member lookup; for relay-wide targets, derive
        `SessionType` from `users.toml` (analogous to send's `has_ui_session`,
        send.rs:482) and apply the capability check.
- [ ] 3.2 In `execute_look` (look.rs:247), remove the
      `runtime_session_type_not_implemented` rejection for `Ui`/`Pubsub` bundle
      members. The capability gate in 3.1 now fires pre-authorization for these
      targets, making the execute-time check unreachable.
- [ ] 3.3 Remove any remaining implicit `@GLOBAL`-specific rejection path in
      look routing that overlaps with the new capability check.

## 4. Update raww handler and routing (BE)

- [ ] 4.1 In `src/relay/handlers/raww.rs`, add a pre-authorization capability
      check. Derive `can_be_written` from the target's `SessionType` and return
      `validation_unsupported_operation` with informative details if false.

      Scoping — same two target paths as task 3.1:
      - **Bundle targets**: `SessionType` from `BundleMember.target.session_type()`.
      - **Relay-wide (`@GLOBAL`) targets**: `prepare_raww` also resolves via
        `bundle.members` (raww.rs:162-174). After task 4.2, a `@GLOBAL` raww
        target passes routing with `ResolvedTarget { relay_wide: true }` and
        hits the member lookup before the capability check. Add an early branch
        on `target_route.relay_wide`; derive `SessionType` from `users.toml`
        (analogous to send's `has_ui_session`, send.rs:482) and apply the
        capability check.

      Implementation note: ACP raww delivers via `session/prompt`, not keystroke
      injection. The `no_enter` parameter is tmux-specific; the existing dispatch
      path in `deliver_one_target_acp` (acp_delivery.rs:437) already handles this
      correctly. No behavioral change needed beyond the capability gate.
- [ ] 4.2 In `execute_raww` (raww.rs:215/252), remove the
      `runtime_session_type_not_implemented` rejection for `Ui`/`Pubsub` bundle
      members. The capability gate in 4.1 fires pre-authorization for these
      targets, making the execute-time check unreachable.
- [ ] 4.3 In `src/relay/routing.rs`, change the `RelayWideTargets::Rejected`
      call to `RelayWideTargets::Allowed` for the look/raww single-target path,
      removing the `@GLOBAL` routing-stage rejection. `@EXTERNAL`/`@RELAY`
      rejections remain unchanged. Then remove the `RelayWideTargets` enum and
      `resolve_target`'s relay-wide-targets parameter entirely — they are dead
      code once the single `Rejected` call site is gone (alpha: no deferral
      needed). Rewrite the `resolve_single_target_route` doc comment to reflect
      the completed transition.

## 5. Rename is_ui in delivery path (BE)

- [ ] 5.1 Rename `is_ui`/`target_is_ui` to `relay_wide` (or
      `relay_wide_target`) in `src/relay/context.rs` and all delivery path
      callers: `handlers/send.rs`, `handlers/raww.rs:236`,
      `delivery/dispatch/payload.rs`, `delivery/dispatch/worker.rs`. No
      behavioral change. Do not use `stream_only`/`stream_delivery` — stream-
      event delivery is decided by `should_route_to_ui` (a strict superset of
      this flag); `relay_wide` aligns with the existing `RouteTarget.relay_wide`
      field in `routing.rs` that directly feeds it (`send.rs:479→493`).

## 6. Tests (BE)

- [ ] 6.1 Add unit/integration tests confirming `validation_unsupported_operation`
      is returned for look and raww against a registered relay-wide session.
- [ ] 6.2 Add tests confirming `@EXTERNAL`/`@RELAY` targets still return
      `validation_unsupported_namespace`.
- [ ] 6.3 Update existing tests whose expected error codes change with this proposal:
      - Tests asserting `validation_unsupported_namespace` for `@GLOBAL` look/raww
        targets → change to `validation_unsupported_operation`
        (`tests/unit/relay_stream/routing.rs:388`).
      - Tests asserting `runtime_session_type_not_implemented` for in-bundle
        `Ui`/`Pubsub` look/raww targets → change to `validation_unsupported_operation`
        (`tests/unit/relay_stream/look.rs:113`; also update the module doc at
        `look.rs:14` which names the old code).
      Confirm that `validation_unsupported_operation` error details include the
      target session id and the relevant capability flag value (e.g.
      `can_be_looked = false`) so the diagnostic is actionable — consistent with
      existing relay error detail patterns.

## 7. Coordination (BE / Coordinator)

- [ ] 7.1 After landing, amend `decouple-transport-layer/tasks.md` to note
      that `can_be_looked`/`can_be_written` capability flags already exist on
      registered sessions and should be incorporated as first-class fields on
      each `TransportImpl` variant rather than re-derived.
- [ ] 7.2 Delete `ideas/relay/6` notebook note (superseded by this proposal).
