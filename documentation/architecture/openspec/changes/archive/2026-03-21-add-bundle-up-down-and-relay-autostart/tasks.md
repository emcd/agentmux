## 1. Contract Design

- [x] 1.1 Modify CLI command topology to include `up` and `down`.
- [x] 1.2 Modify `host relay` contract to no-selector autostart/process-only modes.
- [x] 1.3 Lock `--no-autostart` semantics.
- [x] 1.4 Lock `up/down` selector, idempotence, and `relay_unavailable` semantics.
- [x] 1.5 Lock machine-readable summary payload contracts for host autostart and up/down transitions.

## 2. Runtime/Config Contract

- [x] 2.1 Add optional bundle `autostart` field contract with default false.
- [x] 2.2 Lock no-selector autostart bundle selection semantics.
- [x] 2.3 Preserve existing group naming and trust-boundary semantics for up/down selectors.

## 3. Implementation Follow-up (post-approval)

- [x] 3.1 Update CLI parser/dispatch for `up`/`down` and host no-selector mode.
- [x] 3.2 Implement relay bundle host/unhost control operations.
- [x] 3.3 Implement startup/lifecycle summary payload rendering for new modes.
- [x] 3.4 Add integration tests for autostart, process-only startup, idempotent up/down, and selector validation failures.
- [x] 3.5 Update relevant `data` template(s) with a commented-out `autostart` bundle example.

## 4. Validation

- [x] 4.1 Run `openspec validate add-bundle-up-down-and-relay-autostart --strict`.
