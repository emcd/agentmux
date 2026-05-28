## 1. Contract Design

- [x] 1.1 Add `hosted: bool` to `ListedBundle` payload contract.
- [x] 1.2 Add `ready: bool` to `ListedSession` payload contract.
- [x] 1.3 Lock per-transport readiness predicates (tmux: resolvable pane;
      ACP: `acp_session_ready_for_startup`; ui/pubsub: always false).
- [x] 1.4 Lock bundle hosted predicate: `hosted=true` iff at least one
      configured member is ready.
- [x] 1.5 Lock independence from `state`/`startup_health`/`state_reason_code`.

## 2. Implementation

- [x] 2.1 Add `hosted` and `ready` fields in `src/relay/contract.rs`.
- [x] 2.2 Compute per-member readiness once in `handle_list`; populate
      `ListedSession.ready`; derive `ready_session_count` and `hosted`
      from it. Drop the prior tmux-only `bundle_hosted` short-circuit.
- [x] 2.3 Set `ready: false` and `hosted: false` in the relay-unreachable
      synthesizers in `src/mcp/server.rs` and `src/commands/list.rs`.

## 3. Tests

- [x] 3.1 Update integration fake-relay fixtures
      (`tests/integration/cli/list.rs`, `tests/integration/mcp/list.rs`,
      `tests/integration/runtime_bootstrap.rs`,
      `tests/unit/relay_stream_client.rs`) so payloads include `ready` per
      session and `hosted` per bundle; assert payload values where useful.
- [x] 3.2 Update `tests/unit/relay.rs` regression cases:
      tmux-bearing bundle without owned sessions -> all `ready=false`,
      `hosted=false`; ACP-only bundle without registered worker ->
      all `ready=false`, `hosted=false` (closes prior asymmetry).
- [x] 3.3 Extend `tests/integration/session_relay_list.rs` round-trip:
      pre -> all `ready=false`, `hosted=false`; reconcile -> all
      `ready=true`, `hosted=true`; shutdown -> all `ready=false`,
      `hosted=false`.

## 4. Spec

- [x] 4.1 Add ADDED requirements to `session-relay` delta:
      `Bundle Hosted Flag In List Payload` (any-member-ready predicate)
      and `Per-Session Readiness In List Payload`.

## 5. Validation

- [x] 5.1 Run `openspec validate add-bundle-hosted-in-list-payload --strict`
      (passes).
- [x] 5.2 Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
      and `cargo test` (all clean; 18 lib / 199 integration / 159 unit).
