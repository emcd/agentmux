## 1. Amendment Review

- [ ] 1.1 Tier-1 review of the minting taxonomy, provenance truthfulness,
      kind matrix, and epoch text.
- [ ] 1.2 Tier-2 review of the branch from the architecture and security
      side.
- [ ] 1.3 Reconcile Phase-2 design and runtime-api delta spec with the
      landed amendment text.

## 2. Code and Test Alignment

- [ ] 2.1 Audit registration, record, diagnostic, and attribution sites
      for truthful branch labeling with no elevation either direction.
- [ ] 2.2 Pin the admission matrix (Session/User with enforcement off;
      Application/Relay always token-gated) and the invalidation set
      (replacement, reconnect, disconnect, shutdown, config change) in
      tests.
- [ ] 2.3 Pin home-from-registration and per-operation home-namespace
      authorization for the registration branch in tests.
- [ ] 2.4 Run openspec validate, fmt, clippy, and the full test suite.
