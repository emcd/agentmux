## 1. Enum rename

- [x] 1.1 Rename `AcpWorkerReadinessState` to `WorkerReadinessState` in
      `src/transports/vocabulary.rs`; keep the five variants unchanged.
- [x] 1.2 Update the enum doc comment to describe a transport-agnostic worker
      readiness state (drop "for a persistent ACP worker"; note ACP is one
      populator, Pty the next).
- [x] 1.3 Update re-exports in `src/transports/mod.rs` and any
      `use ... AcpWorkerReadinessState` imports across the crate.

## 2. Registry field + mutator/reader rename

- [x] 2.1 Rename `AsyncWorkerEntry.acp_state` to `readiness` in
      `src/relay/delivery/async_worker.rs` (and the two `acp_state: None`
      initializers).
- [x] 2.2 Rename `set_acp_worker_state` to `set_worker_readiness` and
      `get_acp_worker_state` to `get_worker_readiness`; update internal callers.
- [x] 2.3 Keep one `Option<WorkerReadinessState>` per entry — do NOT introduce a
      transport-keyed readiness map (see design.md).

## 3. In-process observer rename

- [x] 3.1 In `src/relay/delivery/observability.rs`, rename
      `subscribe_acp_worker_state` to `subscribe_worker_readiness`,
      `publish_acp_worker_state` to `publish_worker_readiness`, and
      `WORKER_STATE_PUBLISHERS` to `WORKER_READINESS_PUBLISHERS`.
- [x] 3.2 Update the module doc comment to drop ACP-specific phrasing while
      preserving the pre-registration / post-unregistration subscription
      guarantee.
- [x] 3.3 Update the `mirror_state` closure call site in
      `src/relay/delivery/dispatch/worker.rs` to call `set_worker_readiness`.

## 4. Public read rename

- [x] 4.1 Rename `read_acp_worker_state` to `read_worker_readiness` in
      `src/relay/mod.rs`; preserve the `Option<&'static str>` return and the
      five state strings.
- [x] 4.2 Update the `subscribe_acp_worker_state` re-export in `src/relay/mod.rs`
      to `subscribe_worker_readiness`.

## 5. ACP local references (no behavior change)

- [x] 5.1 Update `WorkerReadinessState` type references in
      `src/acp/{state.rs,transport.rs,worker_driver.rs}` and
      `src/relay/contract.rs` / `src/relay/delivery/dispatch/orchestration.rs`.
- [x] 5.2 Leave `derive_acp_look_snapshot`, its `acp_*` `stale_reason_code`
      strings, and ACP transition triggers unchanged (design.md scope boundary).

## 6. Tests + validation

- [x] 6.1 Update tests that name `read_acp_worker_state`,
      `subscribe_acp_worker_state`, or `AcpWorkerReadinessState`.
- [x] 6.2 `cargo build`, clippy `-D warnings`, and full test run green.
- [x] 6.3 `openspec validate refactor-worker-readiness-interface --strict`.
