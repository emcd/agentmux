## 1. Shared failure-detail plumbing

- [ ] 1.1 Take the lead phrase for the folded reason as a parameter of
  `fold_startup_failures`, keeping the joining and the `failed_sessions` detail
  shape in the one helper, and update the existing all-failed callers to pass
  their current wording.
- [ ] 1.2 Add the recorded per-session failures to `ReconciliationReport`, and
  add nullable structured `details` to `BundleTransitionEntry`.

## 2. Reconcile tolerantly

- [ ] 2.1 Make `reconcile_loaded_bundle` record a failed bootstrap member and
  continue rather than returning, and collect per-member failures from the
  parallel join loop instead of returning on the first error. Keep a worker
  panic, a session-state query failure, and principal registration failure fatal.
- [ ] 2.2 Evaluate readiness in reconcile through the same per-member helper the
  startup path uses, across every configured member rather than only the ones it
  created, so a created-but-not-ready session is a recorded failure and `ready`
  means the same thing it means in the `Bundle Startup Health Model`. Report the
  ready count alongside the recorded failures.
- [ ] 2.3 Persist each recorded reconcile failure through
  `append_startup_failure` so `list` reports the same failures `up` did, and emit
  the existing per-session failure inscription.

## 3. Report the outcome

- [ ] 3.1 Derive `handle_bundle_up`'s outcome from recorded failures and the
  ready count: `degraded` with failures and a ready session, `failed` with
  failures and none ready, otherwise the existing `hosted`/`skipped` result.
- [ ] 3.2 Carry the folded reason and `details.failed_sessions` on the
  transition entry, and add `degraded_bundle_count` plus the widened
  `changed_any` to the relay lifecycle response.
- [ ] 3.3 Stop discarding `failed_startups` when `ready_session_count > 0` in
  `host_selected_bundle`: report `degraded` with the folded reason and details.
- [ ] 3.4 Record the same partial-failure detail on the watcher's load and
  reload paths.
- [ ] 3.5 Add `degraded_bundle_count` to the startup summary and make
  `hosted_any` true when a bundle is degraded.

## 4. Render it

- [ ] 4.1 Render failed session ids and causes in `agentmux up` text output,
  and add `degraded_bundle_count` to its summary line and JSON payload.
- [ ] 4.2 Render the same in the relay host startup summary text output.

## 5. Coverage

- [ ] 5.1 Cover a reconcile in which one member fails and another succeeds:
  the remaining member is attempted, the failure is recorded, and the operation
  does not error.
- [ ] 5.2 Cover the `up` outcome table — degraded with a ready session, failed
  with none ready, and unchanged `hosted`/`skipped` results — including that a
  bundle whose sessions all already exist and are ready still reports `skipped`.
- [ ] 5.6 Cover a created-but-not-ready session alongside another startup
  failure: the not-ready session is recorded as a failure, no session is ready,
  and the outcome is `failed` rather than `degraded`. Cover readiness evaluation
  for an already-running member that reconcile did not create.
- [ ] 5.3 Cover that a non-session-scoped reconcile error still fails the whole
  operation.
- [ ] 5.4 Cover that a partial startup reports `degraded` rather than `hosted`
  in the relay host startup summary, with the folded reason and details present,
  and that `hosted_any` stays true.
- [ ] 5.5 Cover that a failure recorded by `up` is subsequently visible through
  `list`'s `recent_startup_failures`.

## 6. Documentation and verification

- [ ] 6.1 Update the relay and commands subsystem READMEs where they describe
  reconcile failure handling or the bring-up outcome vocabulary.
- [ ] 6.2 Run nextest, clippy, formatting, and strict OpenSpec validation.
- [ ] 6.3 Submit the stack to AuxBE for review.
