## Why

A bundle is a set of sessions, and one session failing to start is not a reason
to withhold the rest. Both paths that bring a bundle up violate that, in
opposite directions, and neither tells the operator which sessions failed.

`agentmux up` aborts the whole reconcile on the first member that fails to
create, after earlier members were already created — a partial effect reported
as a total failure, leaving the operator unable to tell what state the bundle is
in. `agentmux host relay` and the bundle watcher do the reverse: any bundle with
at least one ready session is reported as an unqualified success, and the
per-session failures collected during startup are discarded. Today those reasons
reach the operator only if they separately run `list` or open the TUI.

## What Changes

- `up` reconciles every configured member instead of aborting on the first
  failure, matching the startup path's existing behaviour. Whole-bundle errors
  (catalog miss, principal registration failure, tmux state query failure)
  continue to fail fast.
- Reconcile brings up every configured member through the same per-session
  startup step the startup path uses, rather than treating a successful
  `new-session` as success and never inspecting a member it did not create. A
  created-but-not-ready session becomes a recorded failure, and a member with no
  tmux session to create — an ACP target — is started rather than judged by
  observation. This is what lets the outcome reuse the `degraded` predicate
  rather than a weaker one wearing the same name.
- **BREAKING**: an ACP session whose worker is busy with an in-flight turn is
  reported ready. The startup poll already accepted it; the `list` projection did
  not, so a bundle whose ACP member was mid-turn reported `startup_health=degraded`
  — or `state=down` when it was the only member — for the duration of the turn.
  Making `up`'s outcome share that predicate forced the two to be reconciled, and
  the list side was the wrong one.
- **BREAKING**: the `up`/`down` transition payload gains `outcome=degraded` for
  a bundle that came up with at least one session failing, plus per-entry
  failure detail and a `degraded_bundle_count` aggregate. A consumer treating
  `outcome` as a closed set of `hosted`/`unhosted`/`skipped`/`failed` sees a new
  value.
- **BREAKING**: the relay-host startup summary gains the same `degraded`
  outcome, per-entry failure detail, and `degraded_bundle_count`, replacing the
  unqualified `hosted` it reports for a partial startup today.
- Both CLI renderings name the failed sessions and their causes, rather than
  requiring a follow-up `list`.
- The bundle watcher's load and reload paths record the same partial-failure
  detail they currently discard.

The `degraded` spelling is the one `bundle-lifecycle` already defines for "at
least one session ready and at least one startup attempt failed" and that `list`
already reports as `startup_health`; this change gives the two bring-up surfaces
the same vocabulary rather than inventing a second one.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `cli-surface`: `Relay Host Startup Summary Contract` gains the `degraded`
  outcome, per-bundle failed-session detail, and the `degraded_bundle_count`
  aggregate; `Bundle Lifecycle Transition Summary Contract` gains the same for
  `up`.
- `bundle-lifecycle`: `Relay Bundle Lifecycle Result Contract` gains the
  `degraded` outcome and failed-session detail, and states that a per-session
  startup failure does not fail the bundle transition; `Bundle Reconciliation`
  states that reconcile brings up every configured session through the startup
  path's per-session step; `Bundle Startup Evaluation Boundary` states that one
  readiness predicate per transport serves every surface, and that a busy ACP
  worker is ready.

## Impact

- `src/relay/lifecycle.rs`: `reconcile_loaded_bundle` accumulates per-member
  failures and evaluates per-member readiness through the same helper the
  startup path uses; `ReconciliationReport` carries the failures and the ready
  count.
- `src/relay/contract.rs`: `ReconciliationReport` and `BundleTransitionEntry`
  gain failure detail, reusing `StartupFailureRecord` and
  `fold_startup_failures`.
- `src/relay/handlers/listing.rs`: `handle_bundle_up` derives the transition
  outcome from the reconcile report's failures.
- `src/relay/watcher.rs`: load and reload record partial-failure detail.
- `src/commands/host/relay.rs`, `src/commands/host/summary.rs`: the startup
  summary producer stops discarding `failed_startups`.
- `src/commands/bundle.rs`: `up`/`down` rendering surfaces failed sessions.
- No configuration keys, persisted data, or dependencies change.
