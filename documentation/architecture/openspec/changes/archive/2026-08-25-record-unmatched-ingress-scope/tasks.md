## 1. Receiving-side record

- [x] 1.1 Give `receiving_namespace_discovery` the requesting principal, and emit
      `relay.discovery.namespaces.scope_unmatched` carrying the scope and that
      principal when the scope covers no namespace.
- [x] 1.2 Record in the emitting function why the wire result stays an empty
      success, so the non-disclosure rule is not later "fixed" into a failure.

## 2. Coverage

- [x] 2.1 Assert the new record carries both the scope and the asking principal,
      alongside the unchanged empty response.
- [x] 2.2 Key every inscription assertion to a principal unique to its own test, so
      the assertions stay deterministic when the sink is shared with tests running
      concurrently in the same process.
- [x] 2.3 Give the absence assertion a positive control that is local to its test:
      drive an unmatched scope through the same relay first, so an unwired sink or a
      silent recorder fails the control rather than satisfying the absence.
- [x] 2.4 Confirm each new assertion fails when the behavior it covers is removed.

## 3. Specification

- [x] 3.1 State the recording obligation in Cross-Relay Discovery Ingress
      Filtering, bounded so it cannot be read as licence to change the response.
- [x] 3.2 Add a scenario fixing the aggregate zero-match outcome, which is normative
      in prose today but has no scenario and has twice been reported as a defect.
- [x] 3.3 Run `scripts/verify-openspec-deltas.py record-unmatched-ingress-scope` and
      confirm every reported drop is intended. Reports one added scenario and no
      drops, against a delta seeded from the live requirement rather than retyped.
