## Context

Two code paths bring a bundle up, and they disagree about what a per-session
failure means.

`startup_loaded_bundle` (relay host autostart, and the watcher's load/reload)
iterates configured members, calls `startup_tmux_member`, and on failure pushes a
`StartupFailureRecord` and continues. It produces a `BundleStartupReport` with
`ready_session_count` and `failed_startups`. That is the behaviour the operator
wants everywhere.

`reconcile_loaded_bundle` (the `up` path) calls `create_member_with_retry` and
propagates the first error with `?`, both for the bootstrap member and inside the
parallel join loop. Sessions created before that point stay created, so the
operator is told the transition failed while the bundle is partly up.

The reporting layer then loses what the tolerant path did collect: a bundle with
`ready_session_count > 0` is mapped to `hosted_startup_bundle`, which takes only
a bundle name, so `failed_startups` is dropped at three call sites. The records
are persisted and reach `list`/TUI through `recent_startup_failures`, which is
why the failures are visible at all today — but only to an operator who thinks to
look.

## Goals / Non-Goals

**Goals:**

- Make `up` reconcile every configured member and report per-session failures.
- Make both bring-up surfaces report a partially started bundle as `degraded`
  with the failed session ids and causes inline.
- Keep one vocabulary and one fold for per-session failure detail.
- Keep whole-bundle errors fatal.

**Non-Goals:**

- Change what counts as a session startup failure, or add retry behaviour.
- Change `list`/TUI startup-health reporting, which already distinguishes
  `degraded` correctly.
- Add a configuration key to opt in or out of tolerance.
- Reconcile the `down` path, which has no per-session failure concept.

## Decisions

### Reuse the `degraded` spelling rather than inventing one

`bundle-lifecycle`'s `Bundle Startup Health Model` already defines `degraded` as
"at least one configured session is ready and at least one startup attempt
failed", and `list` already reports it as `startup_health`. Introducing a
different word for the same condition on the bring-up surfaces would leave the
operator correlating two vocabularies for one state. The cost is that `outcome`
grows a variant, which is a breaking change for any consumer treating it as a
closed set — acceptable pre-1.0, and the alternative (a parallel boolean such as
`partial=true` alongside `outcome=hosted`) encodes the same information in a
shape that cannot be rendered or counted uniformly.

### Generalize `fold_startup_failures` rather than adding a second fold

`fold_startup_failures` currently hard-codes its lead sentence to "no configured
session reached ready state (N failed) -- ...", which is false for a partial
startup. Rather than adding a near-duplicate helper, the fold takes the lead
phrase from its caller and keeps ownership of the joining and the
`failed_sessions` detail shape. One producer of the structured detail keeps the
three reporting surfaces from drifting, which is the reason the fold exists.

### Persist reconcile failures through the same history

`up`'s newly recorded failures go through `append_startup_failure`, as the
startup path's already do. Without this, `up` would report a failure that a
following `list` does not know about, and the two surfaces would tell the
operator different stories about the same bundle. `append_startup_failure`
assigns the timestamp and sequence, so the record shape is identical.

### Evaluate readiness in reconcile, and derive the outcome from it

Reusing the `degraded` spelling obliges this change to reuse its predicate.
`Bundle Startup Health Model` defines `degraded` as at least one configured
session **ready** plus a failed startup attempt, and readiness there means what
`startup_tmux_member` checks: `resolve_active_pane_target` succeeds, not merely
that `new-session` returned. Reconcile checks neither — it calls
`create_member_with_retry`, and only for members it found missing, so an
already-running member's readiness is never evaluated at all.

Deriving the outcome from mere presence would therefore report `degraded` for a
bundle the health model would call `down`, using the same word for a weaker
condition. Reconcile instead evaluates every configured member through the same
per-member helper the startup path uses, so a created-but-not-ready session is a
recorded failure rather than a silent success, and `ready` means one thing across
both bring-up paths and `list`.

The outcome then derives from readiness, with `changed` continuing to carry only
creation and pruning so the idempotent `skipped` result is unaffected:

| Failures | Any session ready | Outcome |
|----------|-------------------|---------|
| none | something changed | `hosted` |
| none | nothing changed | `skipped` / `already_hosted` |
| some | yes | `degraded` |
| some | no | `failed` |

Counting creations instead of readiness would misreport a bundle whose sessions
were all already running and ready, since such a member contributes to neither
`created_sessions` nor `failed_sessions`.

### Keep non-session errors fatal

The tolerance is scoped to a named session failing to start. A catalog miss, a
principal registration failure, or a failure of the tmux state query itself are
not attributable to one session and leave the reconcile unable to say what it did
or did not do, so they keep failing the whole operation. This preserves the
project's fail-fast default exactly where it still applies.

## Risks / Trade-offs

- [Consumers reading `outcome` as a closed set] A new `degraded` value can be
  mishandled by a consumer that matches exhaustively → the value is added to
  both contracts in the same change, and `degraded` is defined as a hosted
  outcome so a consumer that treats unknown values as "not failed" stays correct.
- [Silent degradation] Making `up` tolerant means an operator who ignores output
  gets a partly-up bundle where they previously got an error → the failed session
  ids and causes are rendered in `up`'s own text output, which is the point of
  the change; `list` continues to report `startup_health=degraded` afterward.
- [Parallel creation error collection] The reconcile join loop currently returns
  on the first `Err`; collecting instead risks losing a panic distinction →
  worker panics remain a whole-operation failure, separate from a member that
  failed to create.

## Migration Plan

No configuration or persisted-data migration. The startup-failure history format
is unchanged; `up` becomes an additional writer of the same records. Consumers of
the `up`/`down` and startup-summary payloads must accept `outcome=degraded` and
the new `degraded_bundle_count` aggregate.

## Open Questions

None.
