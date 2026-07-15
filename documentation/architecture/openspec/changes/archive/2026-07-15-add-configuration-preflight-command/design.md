## Context

The relay validates a bundle at startup through `load_bundle_configuration`
(public, in `src/configuration`) plus `load_authorization_context` (in
`src/relay/authorization`). A pre-flight command must catch exactly what startup
catches, but the two loaders sit on opposite sides of a visibility boundary:
`load_bundle_configuration` is `pub`, while `load_authorization_context` is
`pub(in crate::relay)` and is the only path that validates policy-control scopes,
session→policy references, `relay.toml` choices range, and `users.toml` policy
mappings. A CLI command in `src/commands` cannot reach the authorization layer
without a relay-side change.

## Goals / Non-Goals

- Goals: a read-only pre-flight whose coverage is identical to relay startup;
  field-level diagnostics (file path + offending field) on failure; fail-fast,
  no partial-load.
- Non-Goals: graceful degradation; aggregating every bundle's errors in one run;
  any change to the configuration loading layer.

## Decisions

- Decision: add a thin public `relay::preflight_bundle_configuration` wrapper
  that calls `load_bundle_configuration` + `load_authorization_context` and
  discards the result. The command depends on the relay's public surface (which
  it already does for `request_relay`, `RelayRequest`, etc.) and reuses the
  literal startup path, so pre-flight and startup cannot drift.
  - Alternatives considered:
    - Configuration-loaders-only check (`load_bundle_configuration` +
      `load_policy_ids`): catches the headline incident and all
      coders/bundle/schema errors, but silently misses policy-control scope
      typos, dangling session→policy references, `relay.toml` range, and
      `users.toml` mappings — narrower than startup, so a "passing" pre-flight
      could still fail to start. Rejected.
    - Widen `load_authorization_context` to `pub`: exposes `AuthorizationContext`
      and its per-bundle context type as public API surface the CLI does not
      need. A thin `()`-returning wrapper keeps that surface internal. Rejected.
- Decision: render the relay error's structured `details` inline in the command.
  The shared `map_relay_error` drops `details`, where the file path and offending
  field live (a bundle parse error maps to `internal_unexpected_failure` with
  `{path, cause}`); dropping it would defeat the command's purpose.
- Decision: read-only — the command does not call
  `ensure_starter_configuration_layout`. A validator must not scaffold or mutate
  the operator's configuration; a missing/empty bundles directory reports
  "no bundle configurations found" rather than creating starter files.

## Risks / Trade-offs

- Fail-fast stops at the first invalid bundle rather than listing every bundle's
  errors. Trade-off accepted per the pre-MVP fail-fast policy: the goal is
  earlier, clearer failure, not exhaustive reporting.

## Migration Plan

Additive only; no existing command, configuration schema, or relay signature
changes. No migration required.
