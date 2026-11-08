## Context

`verify_socket_trust` (identity/verify.rs) admits credential-free
Session/User sessions when enforcement is off, while public
`verify_identity` requires a store credential. The Phase-2 dispatch
boundary needs a named rule for what authorizes those sessions.
Coordinator D1 ruling adopts option (2): an authorized
registration-minted branch. The five-point epoch and bounds
clarification and the no-elevation contract from that ruling bind
this design.

## Goals

- Name registration minting as the explicit authorized rule for the
  credential-free branch, reconciling the 29951774 registration text
  with the runtime-api epoch text as branch-qualified statements.
- Keep every existing behavior: admission matrix, per-operation
  authorization, attribution shape, no store side effects.

## Non-Goals

- No new identity class for Application/Relay kinds.
- No change to credential-minted epoch, revocation, or resurrection
  rules.
- No public constructor or caller trust sentinel.
- No alternate dispatch surface.

## Decisions

- Decision: two-branch sealed input model. Credential-minted
  (DirectPsk plus store match, store-epoch bound) and
  registration-minted (transport admission by authorized
  administrative mutation, instance plus generation-epoch bound)
  share one handler surface. Rationale: formalizes the admitted
  sessions the transport already creates instead of removing or
  redefining them.
- Decision: provenance truthfulness by construction, scoped
  locally. Records, diagnostics, and local attribution label the
  branch; admitted never renders as authenticated. The labeling
  obligation does not cross the relay boundary: remote
  `on_behalf_of` stays advisory and `authenticated_identity`
  stays the independently credential-backed peer, with no new
  wire marker. Rationale: Tier-1 adjudicates this text, and
  mislabeling is the failure the ruling exists to prevent.
- Decision: no elevation either direction and no store side
  effects. Fresh registration is a separate mint; in-place
  provenance upgrades and downgrades are refused by construction.
- Decision: home frozen from admitted registration and bound
  configuration, never wire namespace. Rationale: closes the
  borrowed-home bypass the dispatch maps found.

## Risks / Trade-offs

- [Risk] A future caller mistakes the envelope for authority →
  Mitigation: per-operation authorization and final-admission
  revalidation ignore the envelope except for validity bounds.
- [Risk] Scope creep into Application/Relay credential-free access →
  Mitigation: matrix pinned in spec scenarios; review checklist
  names the kinds explicitly.
