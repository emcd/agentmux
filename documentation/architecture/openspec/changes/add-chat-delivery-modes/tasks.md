## 1. Implementation

- [ ] 1.1 Add `delivery_mode` (`async`|`sync`) to MCP `chat` request schema and
      validation, defaulting to `async`.
- [ ] 1.2 Add optional `quiescence_timeout_ms` to `chat` request validation
      with positive-integer constraints.
- [ ] 1.3 Add async acceptance response contract (`status=accepted`, per-target
      `outcome=queued`) while preserving sync completion contract.
- [ ] 1.4 Introduce relay-side async queueing/worker flow so accepted async
      targets wait indefinitely for quiescence before injection.
- [ ] 1.5 Apply mode-aware timeout defaults and overrides
      (`sync` omitted -> relay sync default, `async` omitted -> no timeout,
      explicit value -> mode-specific bounded wait).
- [ ] 1.6 Keep sync mode blocking behavior with bounded timeout and existing
      delivered/timeout/failed outcomes.
- [ ] 1.7 Add tests for:
      - default async mode when omitted,
      - explicit sync behavior,
      - timeout default/override behavior in both modes,
      - async accepted/queued response shape,
      - sync mixed partial results.
- [ ] 1.8 Update operator/developer documentation for chat mode selection.

## 2. Validation

- [ ] 2.1 Run `cargo check --all-targets --all-features`.
- [ ] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] 2.3 Run `cargo test --all-features`.
