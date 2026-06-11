## Context

Sessions register with the relay over heterogeneous transports — tmux, ACP
stdio, and ui-stream — each with structurally different capabilities. There is
no first-class mechanism today to advertise what operations a given session's
transport supports; instead, operation-validity decisions are scattered across
namespace routing rules and handler logic. The spec's own note acknowledges the
gap: "A richer, transport-class-specific rejection is intentionally deferred to
session-attribute-based routing."

`is_ui` in the relay delivery path is one symptom of the missing capability
layer. It acts as an identity-class proxy for transport metadata and is used in
two distinct ways:

- **Delivery routing** (`send.rs`, `context.rs`, `delivery/dispatch/`): skips
  tmux/ACP transport validation and uses stream-event delivery for relay-wide
  targets.
- **Operation validity** (implicitly, via namespace routing in `routing.rs`):
  `@GLOBAL` targets are rejected with `validation_unsupported_namespace` for
  look and raww because they have no injective transport — but this rejection is
  expressed as a namespace-routing decision, not a capability check.

## Goals / Non-Goals

- Goals: introduce `can_be_looked`, `can_be_written`, and `can_stream_output`
  capability flags on registered sessions, derived from transport type; use
  `can_be_looked`/`can_be_written` in look/raww handler validation; retire the
  deferred-note comment from the spec; rename `is_ui` to clarify
  delivery-routing meaning.
- Non-Goals: restructuring the send delivery path into a `TransportImpl` enum
  (that is `decouple-transport-layer`'s scope); enabling streaming look
  responses for ACP (that is a follow-on proposal using `can_stream_output`);
  changing observable behavior for `@EXTERNAL`/`@RELAY` namespace targets.

## Decisions

- **How capabilities are expressed.** Capabilities are pure functions of
  `SessionType` and SHALL be implemented as derivation — either methods on
  `SessionType` (e.g. `fn can_be_looked(self) -> bool`) or a
  `TransportCapabilities::of(SessionType)` helper — not as stored bool fields.
  Every check site already has a `SessionType` in hand: bundle targets expose it
  via `BundleMember.target.session_type()` from configuration; relay-wide targets
  expose it via `TuiSession::session_type` from `users.toml` (via
  `resolve_global_user_session_type` in `connection.rs`); live registry entries
  expose it via `RegistryEntry.session_type`. No new fields are needed; storing
  bools would duplicate state and introduce initialization-path bugs. There is no
  separate transport subtable; the existing configuration enum discriminants are
  the source of truth. Each transport type carries fixed capabilities:

  | Transport | `can_be_looked` | `can_be_written` | `can_stream_output` |
  |-----------|----------------|----------------|-------------------|
  | `Tmux` | true | true | false |
  | `Acp` | true | true | true |
  | `Pty` | true | true | true |
  | `Ui` | false | false | false |
  | `Pubsub` | false | false | false |

  `Pty` is the long-term replacement for `Tmux` and carries identical
  `can_be_looked`/`can_be_written` capabilities. It sets `can_stream_output =
  true` because PTY natively streams output byte-by-byte; tmux has no
  push-based output protocol and requires repeated look polling.

  `can_stream_output` reflects whether the transport natively produces live
  output chunks: ACP and PTY sessions stream output natively; tmux callers
  observe updates via repeated look calls. The flag is advertised now;
  streaming look semantics are a follow-on proposal.

  Note on `can_be_looked` and scrollback history: the current `look`
  implementation captures the terminal's current visible screen. Scrollback
  history is meaningful only when the wrapped application is not using
  alternative screen mode or ANSI screen clear sequences. Support for
  scrollback-aware look is a tentative future capability for both `Tmux` and
  `Pty` transports and is out of scope for this proposal.

- **When capability is checked.** In the look and raww handler bodies, before
  authorization policy checks — capability is a pre-authorization structural
  check, not a policy decision. The capability is derived from the target's
  `SessionType`:
  - For bundle targets: from `BundleMember.target.session_type()` at the
    prepare step. Note: bundle targets are validated against configuration
    members (not live registry entries); a configured-but-unconnected Tmux
    member is still lookable.
  - For relay-wide (`@GLOBAL`) targets: from `TuiSession::session_type` in
    `users.toml` (config-derived, works whether or not the session is live).
  This replaces both the current namespace-routing `@GLOBAL` rejection AND the
  existing `runtime_session_type_not_implemented` rejections for `Ui`/`Pubsub`
  bundle members in `execute_look`/`execute_raww`. Those call sites are removed;
  every transport-incapable look/raww target returns
  `validation_unsupported_operation` pre-authorization.

- **Error code.** `validation_unsupported_operation` (new). Distinct from
  `validation_unsupported_namespace` (reserved namespace names no session at
  all) and `validation_unknown_target` (session not registered). The new code
  says: the target exists and is routable, but its transport cannot perform
  this operation. This is strictly more informative.

- **`is_ui` rename.** Rename to `relay_wide` (or `relay_wide_target`) in
  `context.rs` and the delivery path. The name `is_ui` was accurate when only
  TUI sessions were relay-wide; it is misleading once we have other relay-wide
  principal types. `stream_only`/`stream_delivery` is not appropriate: stream-
  event delivery is decided by `should_route_to_ui`
  (`delivery/dispatch/payload.rs:319` and `:90`), which is a strict superset
  of this flag — a bundle-bound `TargetConfiguration::Ui` member is also
  stream-delivered with `target_is_ui == false`. Naming the flag `stream_*`
  would assert an implication the code does not have. `relay_wide` aligns with
  the existing `RouteTarget.relay_wide` field in `routing.rs` that directly
  feeds it (`send.rs:479→493`). The rename touches only internal structs, not
  wire format. The delivery-semantics unification belongs in
  `decouple-transport-layer`.

- **`RelayWideTargets` cleanup.** After task 4.2 changes `RelayWideTargets::Rejected`
  to `Allowed` for the look/raww paths, the `Rejected` variant is never
  constructed and the parameter itself is dead. Per alpha delete-dead-code
  policy, remove the `RelayWideTargets` enum and `resolve_target`'s
  relay-wide-targets parameter in this change. The doc comment on
  `resolve_single_target_route` already forecasts this exact change ("the
  blanket relay-wide rejection becomes an attribute check") and should be
  rewritten to reflect the completed transition.

- **`decouple-transport-layer` coordination.** `add-transport-capability-flags`
  must land first so the transport decoupling can incorporate the capability
  derivation as first-class methods on each `TransportImpl` variant from the
  start. After this proposal lands, `decouple-transport-layer/tasks.md` should
  be amended accordingly.

- **Forward note — @GLOBAL non-Ui session types.** If a non-`Ui` session type
  appears in `users.toml` (e.g. `Acp`), the capability gate passes for a
  `@GLOBAL` look, but `execute_look` today has no relay-wide capture path and
  would behave undefined. This cannot happen via current config, but the
  implementation should return a sensible error rather than panic when the gate
  passes for an unhandled relay-wide transport.

- **No behavior change for most clients.** `@EXTERNAL`/`@RELAY` rejections are
  unchanged. Successful look/raww paths are unchanged. The only observable
  change is the error code for `@GLOBAL` look/raww targets
  (`validation_unsupported_namespace` → `validation_unsupported_operation`).
  Alpha: acceptable.

## Risks / Trade-offs

- Any client that pattern-matches on `validation_unsupported_namespace` for
  look/raww `@GLOBAL` rejections will need updating. Alpha: no deployed clients
  pin to this code path.
- The capability flags are intrinsic to the transport type and immutable after
  Hello. There is no mechanism to update them for a live session. This is
  intentional — transport capabilities are structural properties of the wire
  protocol, not policy-configurable decisions.
- **Pre-existing delivery-path coalesce/routing mismatch (deferred).** The
  coalesce predicate in `delivery/dispatch/worker.rs:275/323` gates on the
  narrow `target_is_ui` flag, but payload prep routes on the broader
  `should_route_to_ui` (`payload.rs:90/319`). A `debug_assert` at
  `payload.rs:93` is violatable: two queued sends to a bundle-bound
  `TargetConfiguration::Ui` member both have `target_is_ui == false`, so
  they coalesce, then UI-route in payload prep — debug builds assert,
  release delivers only the head message. This predates this proposal and
  is out of scope; defer to `decouple-transport-layer`. The `relay_wide`
  rename avoids entrenching the mismatch under a misleading `stream_*` name.
