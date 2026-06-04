# Change: Lift cross-bundle restriction on relay Look

## Why

Cross-bundle `Send` already routes by target suffix (`add-cross-namespace-routing`);
`Look` was the next natural read surface a user reaches for when inspecting a
peer bundle, and the TUI already ships `session@bundle` grammar in its
look-target field. Cross-bundle `Look` landed in commit 7f6585a; this change
records the session-relay contract for the behavior that shipped.

## What Changes

- Lift the cross-bundle rejection for `look`. A `look` target qualified with a
  peer bundle (`<session>@<bundle>`) routes to that bundle (suffix-based,
  consistent with cross-bundle `Send`); the snapshot is captured from the peer
  bundle's runtime context.
- The `bundle_name` request field becomes purely redundant: it is the
  dispatch-bundle echo and no longer selects or rejects a peer bundle. The prior
  "reject mismatched bundle name" scenario is retired.
- Retire `validation_cross_bundle_unsupported` for `look` (still in force for
  `raww`, which remains intra-bundle and is out of scope here).
- Add resolution codes: `validation_unknown_bundle` (peer bundle not configured
  on this relay) and `validation_unknown_target` (session not a member of the
  named peer bundle). `Look` distinguishes these two, unlike `Send`, which folds
  both into `validation_unknown_target`.
- **Authorization escalation**: cross-bundle look requires the requester's
  `look` scope to be `all:all`, evaluated against the requester's own (dispatch)
  bundle policy; same-bundle non-self look continues to require `all:home`. The
  self-inspection shortcut applies to same-bundle look only. This deliberately
  does not copy `Send`'s permit-all cross-bundle stance: `all:home` confers no
  authority beyond the requester's own bundle, mirroring the relay-wide
  operator-action posture.
- Target bundles have no inbound say this slice. The deferred target-side
  ingress filter design (whether bundles/relays should declare who may
  look/send/list into them) is recorded in `src/relay/README.md`
  (Authorization model) and notebook `ideas/relay/2`; its forcing function is
  cross-relay routing (v0.8.0), not intra-relay symmetry.

## Impact

- Affected specs: `session-relay`, `cli-surface`, and `mcp-tool-surface`. All
  three carried stale "reject cross-bundle look" text from the same source and
  are reconciled together in this change so the canonical specs stay consistent
  when it lands. (The broader AE parameter/naming audit, todos/mcp/38, is a
  separate pass and is intentionally not folded in here.)
- Cross-cutting note: the `Same-Bundle Stream Scope Enforcement` requirement in
  `session-relay` still describes a blanket cross-bundle frame rejection. That
  wording predates cross-bundle `Send` (the active `add-cross-namespace-routing`
  change has not yet archived its deltas into the canonical spec) and is best
  reconciled holistically rather than piecemeal here. It is consistent with this
  change because cross-bundle `Look` is driven by the `target_session` suffix,
  not by the stream's bound-bundle frame scope.
- Affected code (already merged in 7f6585a): `src/relay/handlers.rs`
  (`handle_look`, new `resolve_look_target_bundle`), `src/relay/authorization.rs`
  (`authorize_look` gained a `cross_bundle` gate); coverage in
  `tests/unit/relay_stream/look.rs`.
