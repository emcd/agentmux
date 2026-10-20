## Why

Isolated executable agent-adapter processes are the confirmed v0.11.0 theme, and the merged `embeddable-runtime-api` settles the in-process runtime boundary — but nothing yet specifies the relay↔adapter wire. The two hard parts (`ideas/transports/5` addendum: Chooser reverse-RPC and OutputView across a process boundary) must be specified as concrete operations before any adapter is built, or the reference implementation will bake in unreviewed protocol decisions.

## What Changes

- Specify the adapter→relay `request_choice` reverse-RPC: generation-bound routing, relay-derived authorization, full `ChoiceMade` taxonomy (including `busy_already_pending`), first-class cancellation, stale-reply invalidation, multiplexing with all other traffic (ACP-authored).
- Specify the relay→adapter poll `look` RPC: stable logical handle, adapter-side prime-wait, generation replies, cursor-compatible schema for additive future streaming (ACP-authored).
- Specify the adapter submit acknowledgment preserving the normative `runtime-api` submission-evidence trichotomy, including `SubmissionUnknown` semantics: declare-before-effect, generation-scoped ids, late/duplicate ack handling, no blind replay (ACP-authored).
- Specify the common wire schema: versioned message envelope, packed-member identity, handshake negotiation fields with invalid-value behavior (ACP-authored from the Backend schema checklist).
- Specify the adapter lifecycle handshake (ordered ACP behavior inside the adapter, neutral ready/failure signals), capabilities/version negotiation, and the opaque resume-mapping custody split (shared ACP + Backend ground).
- Specify the supervisor side: executable configuration, spawn/supervision lifecycle, out-of-band fencing, typed launch/wire failures (Backend-authored).
- Out of scope: the reference adapter implementation and conformance suite (follow-on slice); any change to in-process transport behavior; SDK adoption (`todos/acp/29` stays deferred behind the wire boundary).

## Capabilities

### New Capabilities

- `adapter-choice-rpc`: adapter→relay blocking choice resolution over the wire (ACP-authored).
- `adapter-look-poll`: relay→adapter snapshot reads with prime-wait and freshness (ACP-authored).
- `adapter-submit-acks`: submit acknowledgment preserving the submission-evidence trichotomy (ACP-authored).
- `adapter-lifecycle-handshake`: ordered lifecycle, capabilities/version negotiation, resume-mapping custody (shared ACP + Backend).
- `adapter-supervision`: executable config, spawn/supervision lifecycle, out-of-band fencing, typed launch/wire failures (Backend-authored).
- `adapter-wire-schema`: common message envelope, packed-member identity, handshake negotiation fields with invalid-value behavior (ACP-authored from the Backend schema checklist).

### Modified Capabilities

- None. In-process transport requirements are unchanged by this contract; the `runtime-api` submission-evidence trichotomy is cited, not modified.

## Impact

- Affected code (future implementation slices, not this change): `src/transports/contract` (wire types), `src/acp/*` (adapter host), relay supervisor (Backend lane), configuration schema (new timeout/restart knobs), conformance suite (follow-on).
- Dependencies: merged `embeddable-runtime-api` (normative baseline); `ideas/acp/3` requirements (Tier-1 + Tier-2 approved); `ideas/acp/4` ACP wire-shape draft.
- No breaking change to any shipped surface: the contract is purely additive until an adapter implementation lands.
