## Why

Credential-free socket sessions (`socket-trust` with enforcement off) are
admitted today but have no authorized minting rule: public verification
requires a store credential they lack, so the Phase-2 dispatch boundary
cannot name what authorizes them. Formalizing the existing model
truthfully beats removing it or leaving it spec-unsanctioned (Coordinator
D1 ruling, option 2 adopted).

## What Changes

- Define a registration-minted identity branch alongside the existing
  credential-minted branch: transport admission by authorized
  administrative mutation, bound to runtime instance plus
  connection/registration generation epoch.
- Require truthful non-store-authenticated provenance in every record,
  diagnostic, and attribution for the registration-minted branch, with
  no elevation in either direction and no store side effects.
- Scope the change to the existing admission matrix: Session/User
  principals admitted with `require_session_credentials` off. No
  credential-free branch for Application/Relay kinds.
- Keep per-operation authorization, current-authority-at-final-admission,
  home-namespace resolution, and the single handler surface unchanged.

## Capabilities

### New Capabilities

None — the branch is specified as requirements on the existing identity
capability.

### Modified Capabilities

- `relay-identity`: minting taxonomy (credential-minted vs
  registration-minted), provenance truthfulness, no-elevation and
  no-store-side-effect rules, branch-scoped lifetime and invalidation.

## Impact

- `src/relay/identity/verify.rs` (`verify_socket_trust`), connection
  registration, and the Phase-2 boundary conversion consume the taxonomy;
  no store, provisioning, or handler-surface changes.
- Phase-2 design approval and adapter cutover stay held until this
  amendment lands as a reviewed change.
