# Change: Add transport capability flags to registered sessions

## Why

Sessions register with the relay over heterogeneous transports — tmux, ACP
stdio, and UI stream — each with structurally different capabilities. There is
no first-class mechanism to advertise what operations a session's transport
supports; operation-validity decisions are scattered across namespace routing
rules and handler code. The specs acknowledge this: "A richer,
transport-class-specific rejection is intentionally deferred to
session-attribute-based routing."

The `is_ui` flag on relay delivery targets is one symptom. It does double duty:

1. **Delivery mechanism** — `is_ui = true` means "deliver via stream event"; `is_ui = false` means "deliver via tmux/ACP transport." This is a routing-path property.
2. **Operation validity** — `is_ui = true` is used as a proxy for "this target does not accept `look` or `raww`." This is an operation-capability property.

These two concerns happen to coincide today: every relay-wide (GLOBAL) stream session lacks an injective transport and therefore cannot accept look or raww. But the logic is hardcoded to identity class rather than advertised as transport metadata. A future transport that is relay-wide but terminal-capable (or a bundle-scoped transport that doesn't support look) would have no clean way to express its capabilities without editing handler logic.

This change delivers the deferred capability-advertisement work.

## What Changes

- Add capability derivation for `can_be_looked`, `can_be_written`, and
  `can_stream_output` as methods on `SessionType` (e.g. `fn can_be_looked(self)
  -> bool`) or a `TransportCapabilities::of(SessionType)` helper. These are pure
  functions of the transport type; no new bool fields are stored. The `SessionType`
  for a target comes from `BundleMember.target.session_type()` for bundle members
  or `TuiSession::session_type` (via `users.toml`) for relay-wide targets. The
  capability matrix:

  | Transport | `can_be_looked` | `can_be_written` | `can_stream_output` |
  |-----------|----------------|----------------|-------------------|
  | `Tmux` | true | true | false |
  | `Acp` | true | true | true |
  | `Pty` | true | true | true |
  | `Ui` | false | false | false |
  | `Pubsub` | false | false | false |

  `Pty` is the long-term replacement for `Tmux` and carries identical
  `can_be_looked`/`can_be_written` capabilities. It sets `can_stream_output =
  true` because PTY natively streams output byte-by-byte, unlike tmux which
  requires periodic snapshot polling.

  `can_stream_output` reflects whether the transport natively produces live
  output chunks (ACP streams tool call and assistant output; PTY natively
  streams byte-by-byte; tmux requires periodic snapshot polling). The flag is
  advertised now; streaming look semantics are deferred to a follow-on proposal.

  Note on `can_be_looked` and scrollback history: the current `look`
  implementation captures the terminal's current visible screen. Scrollback
  history is available only when the wrapped application is not using
  alternative screen mode or ANSI screen clear sequences. Support for
  scrollback-aware look is a tentative future capability for both `Tmux` and
  `Pty` transports and is deferred to a follow-on proposal.
- Update `look` handler: after routing resolves the target, check `can_be_looked`.
  Return `validation_unsupported_operation` if false.
- Update `raww` handler: after routing resolves the target, check `can_be_written`.
  Return `validation_unsupported_operation` if false. Remove the current
  namespace-level `@GLOBAL` rejection (which conflates namespace routing with
  operation validity).
- Add `validation_unsupported_operation` error code: the specific,
  capability-based rejection used when a resolved target's transport does not
  support the requested operation.
- Retire the relay-wide single-target routing rejection in `routing.rs` — the
  `RelayWideTargets::Rejected` short-circuit that currently emits
  `validation_unsupported_namespace` for `@GLOBAL` look/raww targets. After
  this change, `resolve_single_target_route` is called with
  `RelayWideTargets::Allowed` for the look and raww paths; the capability gate
  in the handler body is the canonical rejection. Remove the `RelayWideTargets`
  enum and `resolve_target`'s relay-wide-targets parameter in this change — they
  are dead code once the single `Rejected` call site is gone.
- Replace the existing `runtime_session_type_not_implemented` rejections for
  `Ui`/`Pubsub` bundle members in `execute_look` (`look.rs:247`) and
  `execute_raww` (`raww.rs:215/252`) with the unified capability gate. This
  ensures every transport-incapable look/raww target — whether `@GLOBAL` or
  in-bundle — returns `validation_unsupported_operation` pre-authorization.
- Rename `is_ui` in the send delivery path to `relay_wide` (or
  `relay_wide_target`) — it encodes "relay-wide @GLOBAL target, no
  bundle-member transport config," which aligns with the existing
  `RouteTarget.relay_wide` field that feeds it. `stream_only`/`stream_delivery`
  is not appropriate because stream-event delivery is decided by
  `should_route_to_ui`, a strict superset of this flag. Full replacement with a
  `TransportImpl` enum is deferred to `decouple-transport-layer`.

No behavior change for `@EXTERNAL`/`@RELAY` namespace targets; those remain
rejected with `validation_unsupported_namespace` at the routing stage because
they name no registered session at all. For `@GLOBAL` look/raww targets the
error code changes from `validation_unsupported_namespace` to
`validation_unsupported_operation` — a more informative, capability-specific
code. Alpha: acceptable.

## Impact

- Affected specs: `session-relay`
- Affected code: registered session types in `src/relay/`, `src/relay/handlers/look.rs`,
  `src/relay/handlers/raww.rs`, `src/relay/routing.rs`, `src/relay/context.rs`,
  `src/relay/handlers/send.rs`, `src/relay/delivery/dispatch/`
- Coordination: `decouple-transport-layer` should be amended post-landing to
  incorporate the full capability struct as first-class fields on `TransportImpl` variants
- Cross-lane: relay (BE) only; no wire-format changes affecting TUI or MCP
