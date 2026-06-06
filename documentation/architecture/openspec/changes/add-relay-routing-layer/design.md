# Design: Relay Routing/Authz Dispatch Layer

## Context

Source: `designs/relay/7` (full design note with addressing modes, step
sequencing, and open policy questions). Tracks `todos/relay/73` (implement
layer) and unblocks `todos/relay/71` (decompose handlers/authorization).

## Goals / Non-Goals

- Goals: one routing/authz spine for all target operations; cross-bundle reach
  controlled by `CrossBundlePolicy` per operation; handlers implement op body
  only; `todos/relay/76` (cross-bundle raww) resolved as a policy-schema change.
- Non-Goals: async rewrite; changing Send's `PermitAll` cross-bundle policy;
  ingress filter (`ideas/relay/2`, future seam).

## Decisions

### Three-stage model

1. **Resolution** (op-agnostic): parse `@bundle` suffix → catalog lookup →
   `ResolvedTarget { bundle, session_id, transport, runtime }`. Requester's
   dispatch (home) bundle resolved once.
2. **Authorization**: requester controls always from dispatch bundle.
   `CrossBundlePolicy` per operation:
   - Send: `RequireScope(all:all)` (was `PermitAll` in code; spec already required
     this — "Relay Send Scope Control" line 1103 in session-relay)
   - Look: `RequireScope(all:all)` (already correct)
   - Raww: `RequireScope(all:all)` (was `Forbidden`)
   - List: `RequireScope(all:all)` (fixes requester-in-target-authz bug)
3. **Body**: handler receives `ResolvedRoute`; no routing or authz code.

### Addressing modes

- `SingleTarget` (Look, Raww): one resolved target; biggest win — enabling
  cross-bundle becomes a profile flag.
- `MultiTarget` fan-out (Send): N targets across N bundles; per-group
  permission deciders and transport/timeout cross-validation stay in body.
- `BundleEnumerate` (List): no explicit target; routing = which bundle to
  enumerate; cross-bundle list fix comes for free.
- `BundleLevel` / `NoTarget` (Up, Down, Permission ops): use authz stage with
  no resolution targets.
- `RelayWide` (NewPeer, ChangePsk, Identity): subsumed as recognized mode;
  no resolution targets.

### Send enforcement aligns with existing spec

Send's `PermitAll` code behavior is corrected here. The "Relay Send Scope
Control" spec requirement already requires `all:all` for cross-bundle Send
and rejection under `all:home`; the implementation simply never enforced it.
This closes the gap — no spec change needed, only implementation.

## Risks

- **Slice 1 (Send) body concerns**: permission deciders and transport/timeout
  validation stay in the Send body; layer output is a plain resolved-target set.
- **Slice 3 (List) behavior change**: cross-bundle list callers that previously
  received `validation_unknown_sender` will now receive a real result. Flag in
  release notes.
- **Raww policy-schema widening**: widen `raww` allowed-scope set in
  `parse_policy_controls`; operators must explicitly configure `all:all` to
  enable cross-bundle raww — no silent capability grant.
