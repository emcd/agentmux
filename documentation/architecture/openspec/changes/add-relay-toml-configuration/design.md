## Context

The relay already reads `<config-root>/relay.toml` for `[relay.choices]`, but
that shape unnecessarily nests relay settings inside a file that is already
relay-scoped. Other relay-wide settings remain CLI-only. The bundle watcher
design deferred watch-mode configuration to this change, and identity federation
deferred credential-enforcement scope because one relay socket serves every
bundle.
The implementation already treats `--require-credentials` as relay-scoped; the
current bundle-level spec wording is stale and this change converges the spec
with the existing single-socket trust boundary.

## Goals / Non-Goals

- Goals: one durable relay-level config file; fail-fast validation; no silent
  fallback for malformed fields; explicit peer address placeholder for later
  routing work.
- Non-Goals: outbound peer connection management; peer authentication; inbound
  peer relay routing; compatibility shims for removed host flags.

## Decisions

- Decision: use `<config-root>/relay.toml` as the relay configuration table, with
  kebab-case TOML keys at the file root. The choices section becomes
  `[choices] pending-max`; new relay controls are root keys:
  `watch-bundles` and `require-session-credentials`.
- Decision: default `watch-bundles = true` and
  `require-session-credentials = false` when `relay.toml` or either key is
  absent. These defaults preserve current default behavior while moving the
  durable control point to configuration.
- Decision: resolve each relay setting with precedence:
  CLI override > environment override > `relay.toml` > defaults. Existing
  `agentmux host relay --no-watch` and `--require-credentials` remain supported
  as CLI overrides rather than being removed. Environment overrides are
  `AGENTMUX_RELAY_WATCH_BUNDLES` and
  `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS`; both accept only canonical
  boolean strings, `true` and `false`.
- Decision: define top-level `[[peers]]` entries with a required non-empty
  `address` string and no routing behavior. This reserves the operator-facing
  schema needed by outbound peer routing without implying connection attempts,
  health state, identity binding, or target resolution.
- Decision: keep peer PSKs out of `relay.toml`. Raw peer relay PSKs remain
  owner-only state artifacts at `<state-root>/peers/<peer-alias>.psk`; the
  durable principal store records credential hashes, not raw PSKs.
  Actual peer PSK ingest/storage is deferred to the outbound peer routing change;
  this proposal names the existing path convention but does not exercise it.

## Risks / Trade-offs

- Existing `[relay.choices]` operators must migrate to `[choices]` if they have
  customized the choices queue limit. This is an intentional alpha-stage schema
  cleanup while the relay configuration file is still small.
- A schema-only peer table can be mistaken for implemented routing. The spec
  explicitly requires validation and storage only; routing remains out of scope.

## Migration Plan

1. Extend the relay configuration loader to parse all relay-level fields from
   `<config-root>/relay.toml` and expose a normalized runtime configuration.
2. Add environment override parsing and combine overrides in the documented
   precedence order.
3. Thread the resolved values into host startup, watcher spawning, and Hello
   credential verification.
4. Update help/docs/tests to describe flags as overrides, not the durable source
   of truth.
5. Keep configuration pre-flight validation aligned with relay startup.
