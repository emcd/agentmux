## 1. Contract Updates

- [x] 1.1 Add relay requirement for ACP look snapshot ingestion from
      `session/update`.
- [x] 1.2 Lock deterministic bounded retention contract
      (max=1000, oldest-first eviction, oldest->newest ordering).
- [x] 1.3 Add MCP passthrough requirement for ACP look success payloads.
- [x] 1.4 Add CLI passthrough requirement for ACP look success payloads.

## 2. Implementation Follow-up

- [x] 2.1 Implement relay ACP snapshot ingestion/persistence and ACP look
      retrieval path.
- [x] 2.2 Add ACP integration coverage for look ordering and bounded retention.
- [x] 2.3 Update relay unit coverage for ACP look empty-snapshot behavior.

## 3. Validation

- [x] 3.1 Run `openspec validate add-acp-look-transport-behavior-mvp --strict`.
