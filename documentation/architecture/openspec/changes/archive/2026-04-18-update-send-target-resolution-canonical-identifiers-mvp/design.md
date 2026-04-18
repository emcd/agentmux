## Context

Current send routing allows alias-style resolution from configured session
`name` values. This conflicts with deterministic addressing goals and with
future address shape expansions.

## Goals

- Make explicit send targets deterministic and unambiguous.
- Preserve current UI routing capability without alias matching.
- Keep operation-specific validation codes non-contradictory across specs.

## Non-Goals

- Introducing new target address formats (for example `session@bundle`).
- Changing non-send operation target semantics in this change.
- Adding backward-compatibility alias fallback.

## Decisions

1. Canonical explicit-target universe for send in MVP:
   - configured bundle member `session_id`,
   - configured/registered UI session id.
2. Alias/name routing is removed:
   - configured member `name` and display names are not routable.
3. Canonical send reject code:
   - explicit non-canonical or unknown send targets -> `validation_unknown_target`.
4. Validation code unification:
   - `validation_unknown_target` is canonical for unknown/non-canonical target
     tokens across send and non-send target-bearing operations in this relock.
5. Overlap precedence:
   - if token matches both bundle member `session_id` and UI session id, route
     as bundle member target.

## Risks / Trade-offs

- Operators using names instead of ids will see immediate validation failures.
- Client UX may need clearer recipient-picker behavior to avoid name-token
  submission.

## Migration

- Pre-MVP breaking relock is intentional.
- Adapters may still offer name-based search/display, but submission to relay
  must use canonical ids.
