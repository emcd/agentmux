# Change: ACP delivery-side prime timeout

## Why

The Tmux wedge-detection proposal (merged as `tmux-wedge-detection`) introduced
a generic `DeliveryEnvelope.prime_timeout_ms` field and a per-coder
`prime-timeout-ms` config key under `[coders.<id>.tmux]`, and explicitly
deferred the ACP delivery-side prime timeout to this follow-up:

> When the ACP follow-up lands, it will consume the same generic
> `DeliveryEnvelope.prime_timeout_ms` field introduced here, populated from
> `[coders.<id>.acp].prime-timeout-ms` (or a similar per-coder key under the
> ACP table).
>
> — `tmux-wedge-detection/design.md`, Decision 7 / Future Work

ACP delivery-side prime timeout is the missing half of that symmetric surface.
Without it, an ACP target that has gone silent (child process wedged, stdin
buffer full, upstream service hung mid-turn) is waited on indefinitely by the
ACP delivery task's `wait_for_prompt_complete` poll loop. Today the only
bounded outcome is relay shutdown — same shape as the Tmux unbounded behavior
the wedge-detection proposal retires.

ACP wedge detection is a separate concern. Coordinator guidance on this
proposal (2026-06-30): ACP has no prompt-readiness / quiescence gate
equivalent to Tmux's prompt regex. `WorkerReadinessState::Available` is the
SUCCESS state, not a wedge indicator. A clean v1 wedge predicate ("ACP server
not accepting new requests on its stdin channel") requires a probe trait and
test surface that does not exist today. This proposal therefore lands the
prime-timeout half and **defers wedge detection to a separate change**, with
the rationale recorded in `design.md` so the symmetry question is preserved
for future work.

Operator feedback on this proposal (2026-06-30) further tightens the surface
to match Tmux exactly:

1. **Rename the ACP operator knob to `prime-timeout-ms`** to match
   `[coders.<id>.tmux].prime-timeout-ms`. The pre-existing
   `AcpTargetConfiguration.turn_timeout_ms` field is dead baggage today
   (validated, never consumed). This proposal renames it to
   `prime_timeout_ms` so the operator vocabulary is symmetric across
   the two transports. The previous proposal revision kept the legacy
   name; operator direction supersedes that.
2. **Drop the ACP-specific per-call surfaces from MCP `send` and CLI**
   entirely. `--acp-turn-timeout-ms` and `acp_turn_timeout_ms` were the
   only transport-specific fields remaining on generic interfaces after
   `tmux-wedge-detection` retired the Tmux per-call flag. Operator
   direction is to retire them as well so v1 is fully config-only across
   both transports. Existing CLI/MCP invocations that pass the flag/field
   will fail at the parser as unknown.

The combination lands a fully symmetric surface: both transports use the
same operator key (`prime-timeout-ms`), the same envelope field
(`DeliveryEnvelope.prime_timeout_ms`), the same outcome vocabulary
(`SendOutcome::Timeout` + per-transport reason code), and the same
config-only v1 posture. No transport-specific field on any generic
interface survives in v1.

## What Changes

- Add bounded ACP prime-timeout behavior by introducing the
  `prime_timeout_ms: Option<u64>` field on `AcpTargetConfiguration`
  (renamed from the pre-existing `turn_timeout_ms`). The new TOML
  key is `prime-timeout-ms` under `[coders.<id>.acp]`, matching
  `[coders.<id>.tmux].prime-timeout-ms` exactly. When the prime
  window elapses during the wait for a prompt completion with no
  terminal ACP response and no choice pending, the flush group
  resolves as `SendOutcome::Timeout` with
  `reason_code = "acp_turn_timeout"` (matching the existing
  session-relay ACP Stop-Reason Outcome Mapping requirement).
- Wire the per-coder config value into the new generic
  `DeliveryEnvelope.prime_timeout_ms` field introduced by
  `tmux-wedge-detection`. The relay populates this field from
  `[coders.<id>.acp].prime-timeout-ms` at envelope construction
  time for ACP sessions. No per-call override exists in v1
  (the previous `--acp-turn-timeout-ms` flag and `acp_turn_timeout_ms`
  MCP payload field are retired — see "Drop per-call override
  surfaces" below).
- The ACP delivery task (`src/acp/transport.rs::acp_delivery_task`)
  consumes `DeliveryEnvelope.prime_timeout_ms` to bound the
  `wait_for_prompt_complete` poll loop for one turn. The prime timer
  starts at the moment the prompt is dispatched (first wait start)
  and does NOT reset on coalesce-during-wait when new envelopes are
  absorbed into the flush group (matching the Tmux-side anchor at
  "delivery task perspective, not enqueue time" per
  `tmux-wedge-detection` Decision).
- When the prime timer fires with no terminal completion and no
  outstanding pending choice, the ACP transport resolves every
  sender in the flush group with `SendOutcome::Timeout` and
  `reason_code = "acp_turn_timeout"`. The transport does NOT inject
  further messages into the wedge; the failure is terminal and the
  relay's per-call diagnostic inscription records the elapsed time
  and prime bound.
- **Drop per-call override surfaces.** The pre-existing per-call
  override paths on the generic `send` interface are retired:
  - CLI: `--acp-turn-timeout-ms <MS>` is removed. The CLI parser
    rejects the flag as unknown.
  - MCP `send` payload: `acp_turn_timeout_ms` is removed. The MCP
    server rejects the field as unknown.
  The retirement is symmetric with `tmux-wedge-detection`'s
  retirement of `--quiescence-timeout-ms` and `quiescence_timeout_ms`
  for Tmux: v1 of both transports is fully config-only. No
  transport-specific timeout field remains on the generic `send`
  interface in v1. The rejections are generic unknown-argument
  errors; the proposal does NOT require the parser or schema to
  name a migration path. Alpha defaults apply: operators who hit
  the rejection consult the changelog.
- ACP wedge detection is **deferred** to a separate change. The
  rationale (recorded in `design.md` Decision 3) is that a clean
  wedge predicate requires an ACP stdin-acceptability probe trait
  that does not exist today, and `WorkerReadinessState::Available` is
  the success state — not a wedge indicator. This proposal does NOT
  add a normative ACP wedge requirement to `session-relay/spec.md`.
- Add a new `session-relay` requirement: "ACP Prime Timeout" — the
  normative surface for the prime-timeout half of the ACP parity.
  The requirement references the existing `ACP Stop-Reason Outcome
  Mapping` `acp_turn_timeout` reason code so the outcome vocabulary
  is shared (no new `SendOutcome` variant).
- Add a new `transport-abstraction` requirement: "ACP Prime Timeout
  Envelope Field Consumption" — the ACP transport's obligation to
  consume the generic `DeliveryEnvelope.prime_timeout_ms` field.
  This is a sibling requirement to the existing
  `Prime Timeout Envelope Field` requirement added by
  `tmux-wedge-detection`, scoped to the ACP consumer side.
- MODIFY the `cli-surface` `Send Timeout Override Flags by Transport`
  requirement to remove the per-call ACP timeout flag entirely.
  v1 has no transport-scoped timeout override flags; both
  `--quiescence-timeout-ms` (retired by `tmux-wedge-detection`) and
  `--acp-turn-timeout-ms` (retired by this proposal) are rejected
  as unknown flags. The rejections are generic; the proposal does
  NOT require the parser to name a replacement.
- MODIFY the `mcp-tool-surface` `Send Target Selection` requirement
  to remove the per-call ACP timeout payload field entirely. v1
  has no transport-scoped timeout override fields; both
  `quiescence_timeout_ms` (retired by `tmux-wedge-detection`) and
  `acp_turn_timeout_ms` (retired by this proposal) are rejected as
  unknown fields. The rejections are generic; the proposal does
  NOT require the server to name a replacement.
- MODIFY the `session-relay` `Non-Expiring Choice Pending Lifecycle`
  requirement to remove the parenthetical cross-reference to
  `acp_turn_timeout_ms` and `[coders.acp] turn-timeout-ms` (both
  retired). The independence claim is preserved under the new
  field name (`[coders.acp] prime-timeout-ms`).

## Impact

- Affected specs: `session-relay`, `transport-abstraction`, `cli-surface`,
  `mcp-tool-surface`.
- Affected code:
  - `src/acp/transport.rs` — consume `DeliveryEnvelope.prime_timeout_ms`
    inside `acp_delivery_task` to bound the `wait_for_prompt_complete`
    poll loop; record `delivery_prime_timeout` inscription; resolve
    the flush group with `SendOutcome::Timeout` +
    `reason_code = "acp_turn_timeout"` on fire.
  - `src/relay/delivery/dispatch/worker.rs::build_coder_envelope` —
    populate `DeliveryEnvelope.prime_timeout_ms` from
    `[coders.<id>.acp].prime-timeout-ms` for ACP sessions. The new
    field replaces the existing `quiescence_timeout` field on the
    envelope (already removed by `tmux-wedge-detection`); for ACP
    tasks the relay copies the configured prime bound into the new
    generic field.
  - `src/relay/delivery/quiescence.rs` — `QuiescenceOptions` gains a
    `prime_timeout_ms: Option<u64>` constructor parameter (added by
    `tmux-wedge-detection`); the ACP path populates it from
    `AcpTargetConfiguration.prime_timeout_ms`.
  - `src/configuration/types.rs`,
    `src/configuration/raw.rs`,
    `src/configuration/targets.rs` — rename
    `turn_timeout_ms` → `prime_timeout_ms` across the typed config,
    raw loader, intermediate loader, builder, and validator. The
    validator's error message updates from "ACP turn-timeout-ms must
    be greater than zero" to "ACP prime-timeout-ms must be greater
    than zero."
  - `src/relay/handlers/send.rs` (and `raww.rs` if applicable) —
    drop the per-call `acp_turn_timeout_ms` payload read path.
    Today this field is read into the task's quiescence options;
    after this proposal the field does not exist, so the read is
    removed and the `QuiescenceOptions::for_async` constructor
    takes only `quiet_window_ms`.
  - `tests/integration/acp/lifecycle.rs` and
    `tests/integration/acp/helpers.rs` — extend the ACP integration
    surface with a configurable prime timeout test
    (`acp_prime_timeout_fires_after_configured_window`) and a
    coalesce-during-prime test (`acp_prime_timer_does_not_reset_on_coalesce`).
- Breaking changes (alpha defaults apply per AGENTS.md "Alpha Defaults":
  this project is alpha software with live releases; do not preserve
  backwards compatibility unless the human developer explicitly
  requests it):
  - Bundle config key rename: operators with `[coders.<id>.acp]
    turn-timeout-ms` set will see a `deny_unknown_fields` error from
    the raw loader on next bundle load. Operator direction
    (2026-06-30) accepts this break in service of symmetric operator
    vocabulary across the two transports.
  - CLI flag retirement: `--acp-turn-timeout-ms` invocations will
    fail at the CLI parser as unknown. Operator direction accepts
    this break; the existing per-call surface is retired in service
    of fully generic `send` interfaces.
  - MCP payload field retirement: `acp_turn_timeout_ms` payloads
    will be rejected as unknown. Operator direction accepts this
    break; same rationale.
- Out of scope (deferred to a separate OpenSpec): ACP delivery-side
  wedge detection. The rationale is recorded in `design.md` Decision 3
  and Future Work.

## Amendment history

This proposal is filed as the explicit follow-up to the
`tmux-wedge-detection` amendment (Decision 7 / Future Work, commit
`773f3d4`). It addresses Coordinator feedback that the original
proposal deferred ACP work "without an active SHALL requirement in
`session-relay/spec.md`." That gap is closed here for the prime-timeout
half. Wedge detection is explicitly deferred again — this time with a
specific rationale (no clean v1 predicate) rather than as a generic
future-work note.

The first revision of this proposal kept the pre-existing
`turn_timeout_ms` operator knob and the pre-existing per-call override
surfaces (`--acp-turn-timeout-ms`, `acp_turn_timeout_ms`). Operator
feedback (2026-06-30) asked for a tighter surface: rename the operator
knob to `prime-timeout-ms` for cross-transport symmetry, and drop the
per-call override surfaces entirely so v1 is fully config-only across
both transports. This revision implements both operator directions.