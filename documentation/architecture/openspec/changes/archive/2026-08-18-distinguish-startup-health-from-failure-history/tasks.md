## 1. Coverage

- [x] 1.1 Coverage that a bundle carrying a persisted startup-failure record
  reports `startup_health=healthy` when every configured session is ready. This
  is the one direction from which the non-coupling of health and history can be
  demonstrated rather than merely asserted: an implementation that read the
  history to decide health would report `degraded` here
- [x] 1.2 Coverage that a successful delivery to a session clears its persisted
  startup-failure records, driven through the delivery path rather than by
  calling the clearing helper directly. The delta names two observations that
  clear a record, and the delivery-side one is the trigger no test exercises as
  a delivery

## 2. Documentation

- [x] 2.1 Reconcile `src/relay/README.md`'s startup-failure section with the
  spec's vocabulary once the delta lands, so the subsystem note and the
  requirement describe the record's lifetime in the same terms
