## 1. Contract Design

- [ ] 1.1 Add relay `about` operation contract with same-bundle selector semantics.
- [ ] 1.2 Lock canonical response schema for `about` and CLI/MCP parity.
- [ ] 1.3 Lock validation/error semantics (`validation_unknown_bundle`, `validation_unknown_session`, `validation_cross_bundle_unsupported`).
- [ ] 1.4 Lock authorization mapping for `about` to `list.read` and denial passthrough requirements.

## 2. Configuration Model Contract

- [ ] 2.1 Add optional bundle `description` field contract.
- [ ] 2.2 Add optional session `description` field contract.
- [ ] 2.3 Lock description normalization and limits:
  - bundle <= 2048 UTF-8 characters
  - session <= 512 UTF-8 characters
  - trim leading/trailing whitespace
  - reject whitespace-only as `validation_invalid_description`
  - preserve internal newlines.

## 3. Implementation Follow-up (post-approval)

- [ ] 3.1 Update configuration parsing/validation for description fields.
- [ ] 3.2 Add relay request/response handling for `about`.
- [ ] 3.3 Add CLI `about` command wiring and machine output mapping.
- [ ] 3.4 Add MCP `about` tool wiring and payload mapping.
- [ ] 3.5 Add tests for selector failures, auth-deny passthrough, ordering, and null serialization.

## 4. Validation

- [ ] 4.1 Run `openspec validate add-about-surface-and-description-fields --strict`.
